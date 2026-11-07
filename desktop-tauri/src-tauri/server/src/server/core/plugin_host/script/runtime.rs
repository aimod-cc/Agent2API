use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::storage::{Package, Registry};
use crate::server::core::account_store::AccountStore;
use crate::server::core::plugin_host::{self, Host, HostOptions, RpcAdapter, StoreCallbacks};

fn owners() -> &'static Mutex<HashMap<String, String>> {
    static OWNERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn package_for_provider(provider: &str) -> Option<String> {
    if let Some(id) = owners().lock().ok()?.get(provider).cloned() {
        return Some(id);
    }
    super::storage::read()
        .ok()?
        .packages
        .into_iter()
        .find(|package| package.enabled && package.providers.iter().any(|id| id == provider))
        .map(|package| package.id)
}

pub fn registered_label(provider: &str) -> Option<String> {
    known_provider(provider).then(|| provider_label(provider))
}

pub fn known_provider(provider: &str) -> bool {
    super::catalog::label(provider).is_some()
        || super::storage::read().ok().is_some_and(|registry| {
            registry
                .packages
                .iter()
                .any(|package| package.providers.iter().any(|id| id == provider))
        })
}

pub fn provider_state(provider: &str) -> Option<&'static str> {
    let registry = super::storage::read().ok()?;
    let package = registry
        .packages
        .iter()
        .find(|package| package.providers.iter().any(|id| id == provider))?;
    Some(if !package.enabled {
        "pack_disabled"
    } else if package.disabled_providers.iter().any(|id| id == provider) {
        "provider_disabled"
    } else {
        "not_loaded"
    })
}

pub fn clear_owners() {
    if let Ok(mut saved) = owners().lock() {
        saved.clear();
    }
}

pub fn host() -> Option<Arc<Host>> {
    plugin_host::runtime::host_for_pack(super::HOST_ID)
}

pub fn concurrency_default(provider: &str) -> u64 {
    if !plugin_host::runtime::host_for_provider(provider).is_some_and(|host| host.options.script) {
        return 0;
    }
    plugin_host::account_features_of(provider)
        .and_then(|features| features.concurrency_default)
        .map(|limit| u64::from(limit.min(1000)))
        .unwrap_or(0)
}

pub fn observe_concurrency(host: &Host, provider: &str, value: &Value) {
    let Some(limit) = value.get("concurrency_default").and_then(Value::as_u64).filter(|limit| *limit > 0 && *limit <= 1000) else { return };
    if let Ok(mut infos) = host.providers.lock() {
        if let Some(features) = infos.iter_mut().find(|info| info.id.as_str() == provider)
            .and_then(|info| info.account_features.as_mut())
        {
            features.concurrency_default = Some(limit as u32);
        }
    }
}

pub fn snapshot() -> Value {
    let registry = super::storage::read();
    let current = host().map(|host| host.status());
    let items: Vec<Value> = registry.as_ref().map(|registry| registry.packages.iter().map(|package| {
        json!({
            "id": package.id, "name": package.name, "version": package.version,
            "source": package.source, "checkinAllowed": package.checkin_allowed,
            "enabled": package.enabled, "installedAt": package.installed_at, "error": package.error,
            "providers": package.providers.iter().map(|id| json!({
                "id": id, "label": provider_label(id),
                "enabled": !package.disabled_providers.contains(id),
                "loaded": package.enabled && plugin_host::serves_provider(id) && package_for_provider(id).as_deref() == Some(package.id.as_str()),
            })).collect::<Vec<_>>(),
            "running": package.enabled && current.as_ref().is_some_and(|state| state.running),
        })
    }).collect()).unwrap_or_default();
    json!({
        "runtime": super::bun::snapshot(), "packages": items, "operation": super::operations::operation(),
        "error": registry.err(), "hostError": current.and_then(|state| state.last_error),
        "recommendations": super::catalog::recommendations(),
        "recommended": { "name": super::MINIMAX_PACKAGE, "version": super::MINIMAX_VERSION,
            "label": "MiniMax Code 国内／国际", "providers": ["minimax-code", "minimax-code-global"] },
    })
}

pub fn provider_label(id: &str) -> String {
    super::catalog::label(id).map(str::to_string).unwrap_or_else(|| {
        plugin_host::lookup(id).map(|adapter| adapter.label().to_string()).unwrap_or_else(|| id.to_string())
    })
}

pub async fn start(store: AccountStore) -> Result<(), String> {
    let mut registry = super::storage::read()?;
    if registry.packages.iter().all(|package| !package.enabled) {
        return Ok(());
    }
    let executable = super::bun::ensure().await?;
    if let Err(error) = start_registry(&store, &mut registry, executable).await {
        let _ = plugin_host::runtime::shutdown_packs(&[super::HOST_ID.to_string()]).await;
        super::transport::clear();
        for package in registry
            .packages
            .iter_mut()
            .filter(|package| package.enabled)
        {
            package.error = Some(error.clone());
        }
        super::storage::save(&registry)?;
        return Err(error);
    }
    super::storage::save(&registry)
}

pub async fn start_registry(
    store: &AccountStore,
    registry: &mut Registry,
    executable: std::path::PathBuf,
) -> Result<(), String> {
    let prepared = prepare(store, registry, executable).await?;
    activate(registry, prepared)
}

pub struct PreparedRuntime {
    host: Option<Arc<Host>>,
    callbacks: Option<Arc<StoreCallbacks>>,
    infos: Vec<proto::ProviderInfo>,
    owners: HashMap<String, String>,
}

pub async fn prepare(
    store: &AccountStore,
    registry: &mut Registry,
    executable: std::path::PathBuf,
) -> Result<PreparedRuntime, String> {
    let enabled: Vec<Package> = registry
        .packages
        .iter()
        .filter(|package| package.enabled)
        .cloned()
        .collect();
    if enabled.is_empty() {
        return Ok(PreparedRuntime {
            host: None,
            callbacks: None,
            infos: Vec::new(),
            owners: HashMap::new(),
        });
    }
    let mut entries = Vec::new();
    for package in &enabled {
        entries.push(
            json!({ "id": package.id, "directory": super::storage::directory(package)?,
            "options": package.options, "providers": package.providers, "checkinAllowed": package.checkin_allowed }),
        );
    }
    let script = super::storage::host_files()?;
    let options = HostOptions {
        args: vec![
            "run".into(),
            "--no-env-file".into(),
            script.to_string_lossy().into_owned(),
        ],
        cwd: script.parent().map(std::path::Path::to_path_buf),
        data_dir: super::root().display().to_string(),
        init_timeout: Duration::from_secs(60),
        loopback_port: plugin_host::bootstrap::loopback_port(),
        script: true,
        init_payload: json!({ "packages": entries }),
        ..HostOptions::new(executable)
    };
    let callbacks = Arc::new(StoreCallbacks::new(store.clone()));
    let host = Arc::new(Host::new(options, callbacks.clone()));
    plugin_host::runtime::install_pack(
        super::HOST_ID,
        plugin_host::ProviderCategory::Agent,
        host.clone(),
    );
    let infos = host.ensure().await?;
    let discovered = host
        .call_coded("script.packages", json!({}), Duration::from_secs(10))
        .await
        .map_err(|error| error.message)?;
    let list = discovered.as_array().ok_or("脚本宿主没有返回插件清单")?;
    let native_owners = plugin_host::ownership::owners(&plugin_host::PackRegistry::load());
    let mut next_owners = HashMap::new();
    for package in registry
        .packages
        .iter_mut()
        .filter(|package| package.enabled)
    {
        let found = list
            .iter()
            .find(|item| item["id"].as_str() == Some(package.id.as_str()))
            .ok_or("脚本宿主遗漏已启用的插件")?;
        package.error = found["error"].as_str().map(str::to_string);
        if package.error.is_some() {
            continue;
        }
        let providers = found["providers"]
            .as_array()
            .ok_or("脚本插件提供商清单无效")?;
        let mut ids = Vec::new();
        for provider in providers {
            let id = provider.as_str().ok_or("脚本插件提供商标识无效")?;
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
                || crate::server::core::providers::availability::is_suspended(id)
                || crate::server::core::providers::availability::BUILTIN_SUBSCRIPTION_IDS
                    .contains(&id)
                || native_owners.contains_key(id)
                || id.starts_with("custom-")
                || plugin_host::runtime::host_for_provider(id)
                    .is_some_and(|owner| !Arc::ptr_eq(&owner, &host))
                || next_owners
                    .insert(id.to_string(), package.id.clone())
                    .is_some()
            {
                return Err(format!("脚本插件提供商 {id} 与现有身份冲突或尚未开放"));
            }
            ids.push(id.to_string());
        }
        if ids.is_empty() {
            return Err("插件没有可兼容的提供商入口".into());
        }
        package.providers = ids;
    }
    let mut declared: Vec<String> = infos.iter().map(|info| info.id.0.clone()).collect();
    let mut discovered: Vec<String> = next_owners.keys().cloned().collect();
    declared.sort();
    discovered.sort();
    if declared != discovered {
        return Err("脚本握手与包清单的提供商身份不一致".into());
    }
    if !host.ready() {
        return Err("脚本宿主在完成加载前已退出".into());
    }
    Ok(PreparedRuntime {
        host: Some(host),
        callbacks: Some(callbacks),
        infos,
        owners: next_owners,
    })
}

pub fn activate(registry: &Registry, prepared: PreparedRuntime) -> Result<(), String> {
    let PreparedRuntime {
        host,
        callbacks,
        infos,
        owners: next_owners,
    } = prepared;
    let Some(host) = host else {
        clear_owners();
        return Ok(());
    };
    if !host.ready() {
        return Err("脚本宿主在完成加载前已退出".into());
    }
    let active_ids: Vec<String> = registry
        .packages
        .iter()
        .filter(|package| package.enabled && package.error.is_none())
        .flat_map(|package| {
            package
                .providers
                .iter()
                .filter(|id| !package.disabled_providers.contains(id))
                .cloned()
        })
        .collect();
    // 只在网关完成身份冲突校验后授予账号权限，初始化脚本不能读取既有凭证。
    if let Some(callbacks) = callbacks {
        callbacks.authorize_providers(next_owners.keys().cloned().collect());
    }
    let mut adapters = Vec::new();
    for mut info in infos {
        let id = info.id.0.clone();
        if !active_ids.contains(&id) {
            continue;
        }
        if let Some(label) = super::catalog::label(&id) {
            info.label = label.into();
        }
        plugin_host::rpc_cache::set_defaults(&id, info.default_models.clone());
        let kind = crate::server::core::providers::kind_from_id(&id)
            .unwrap_or(crate::server::core::providers::ProviderKind::WorkBuddy);
        adapters.push((
            id.clone(),
            Arc::new(RpcAdapter::new(id, kind, host.clone(), info)),
        ));
    }
    plugin_host::runtime::set_providers(super::HOST_ID, active_ids);
    if let Ok(mut saved) = owners().lock() {
        *saved = next_owners;
    }
    plugin_host::registry::stage_all(adapters);
    crate::server::core::providers::adapter::restore_cached_catalogs();
    crate::server::core::checkin_schedules::extend_plugin_providers_once();
    plugin_host::schedules::wake();
    Ok(())
}

pub fn all_ids(registry: &Registry) -> std::collections::HashSet<String> {
    registry
        .packages
        .iter()
        .flat_map(|package| package.providers.iter().cloned())
        .collect()
}
