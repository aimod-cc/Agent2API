//! 插件下载任务在服务端执行，进度按包 id 隔离；下载后自动验签、安装并热加载。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};

use super::{catalog, packages, update};
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::logging;

mod activation;
mod management;
mod policy;
pub use management::{change, PackChange};
use policy::InstallPolicy;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub action: String,
    pub stage: String,
    pub active: bool,
    pub done: bool,
    pub received: u64,
    pub total: u64,
    pub percent: Option<f64>,
    pub version: String,
    pub required: bool,
    pub required_version: Option<String>,
    pub error: Option<String>,
    pub host_error: Option<String>,
    pub updated_at: i64,
    pub result: Option<Value>,
}

fn tasks() -> &'static Mutex<BTreeMap<String, Operation>> {
    static TASKS: OnceLock<Mutex<BTreeMap<String, Operation>>> = OnceLock::new();
    TASKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn mutation_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

pub async fn mutation_guard() -> MutexGuard<'static, ()> {
    mutation_lock().lock().await
}

fn record_key(id: &str) -> String {
    format!("pluginOperation:{id}")
}

fn mutate(id: &str, update: impl FnOnce(&mut Operation)) {
    if let Ok(mut tasks) = tasks().lock() {
        if let Some(task) = tasks.get_mut(id) {
            update(task);
            task.updated_at = logging::now_ms();
        }
    }
}

fn persist(id: &str) {
    let operation = tasks().lock().ok().and_then(|tasks| tasks.get(id).cloned());
    if let Some(operation) = operation {
        if let Ok(value) = serde_json::to_value(operation) {
            let _ = task_state::store_value(&record_key(id), value);
        }
    }
}

pub(super) fn set_stage(id: &str, stage: &str) {
    let changed = tasks().lock().map(|mut tasks| {
        let Some(task) = tasks.get_mut(id) else { return false };
        if task.stage == stage { return false; }
        task.stage = stage.to_string();
        task.percent = None;
        task.updated_at = logging::now_ms();
        true
    }).unwrap_or(false);
    if changed { persist(id); }
}

pub fn active(id: &str) -> bool {
    tasks()
        .lock()
        .ok()
        .and_then(|tasks| tasks.get(id).map(|task| task.active))
        .unwrap_or(false)
}

pub fn snapshot() -> Value {
    let mut operations = tasks()
        .lock()
        .map(|tasks| tasks.clone())
        .unwrap_or_default();
    if let Ok(states) = task_state::read_all() {
        for (key, state) in states {
            let Some(id) = key.strip_prefix("pluginOperation:") else {
                continue;
            };
            if operations.contains_key(id) {
                continue;
            }
            let Some(value) = state.value else {
                continue;
            };
            let Ok(mut operation) = serde_json::from_value::<Operation>(value) else {
                continue;
            };
            if operation.active {
                operation.active = false;
                operation.stage = "failed".to_string();
                operation.error = Some("上次插件安装中断，请重试".to_string());
            }
            operation.result = None;
            operations.insert(id.to_string(), operation);
        }
    }
    json!({ "operations": operations.into_values().collect::<Vec<_>>() })
}

pub async fn install_prepared(prepared: packages::PreparedPack) -> Result<Value, String> {
    let id = prepared.id().to_string();
    let guard = claim_operation(&id, InstallPolicy::Manual)?;
    let operation = Operation {
        id: id.clone(), action: "install".to_string(), stage: "waiting".to_string(), active: true,
        version: prepared.record(&super::PackRegistry::load()).version,
        updated_at: logging::now_ms(), ..Default::default()
    };
    tasks().lock().map_err(|_| "插件任务状态不可用".to_string())?.insert(id.clone(), operation);
    persist(&id);
    crate::spawn_task(async move {
        let result = activation::install(prepared, None).await;
        finish_operation(&id, result.as_ref(), guard, InstallPolicy::Manual);
        result
    }).await.map_err(|error| format!("插件安装任务中断：{error}"))?
}

pub fn start_cached(
    id: &str,
    action: &str,
    target: &str,
    version: &str,
    platform: &Value,
    automatic: bool,
) -> Result<Operation, String> {
    if target != super::host_target() {
        return Err("只能安装当前网关平台的插件包".to_string());
    }
    let asset = update::PackAsset {
        version: version.to_string(),
        name: update::asset_name(id, version, target),
        url: platform["url"].as_str().unwrap_or("").to_string(),
        size: platform["size"].as_u64().unwrap_or(0),
        page: String::new(),
        require_signed: automatic || platform["requireSigned"] == true,
    };
    let policy = if automatic {
        InstallPolicy::Automatic(super::settings::current().channels())
    } else {
        InstallPolicy::Manual
    };
    start_asset(id, action, asset, policy)
}

pub(super) fn start_required(
    id: &str,
    mut asset: update::PackAsset,
    automatic: bool,
) -> Result<Operation, String> {
    asset.require_signed = true;
    start_asset(id, "repair", asset, InstallPolicy::Required { automatic })
}

fn claim_operation(id: &str, policy: InstallPolicy) -> Result<task_state::RunGuard, String> {
    if active(id) { return Err("这个插件正在下载或安装".to_string()); }
    match task_state::claim(
        &record_key(id), 60_000, true,
        if policy.automatic() || matches!(policy, InstallPolicy::Required { .. }) {
            ManualBackoff::Respect
        } else { ManualBackoff::Bypass },
        1000,
    )? {
        Claim::Acquired(guard) => Ok(guard),
        Claim::Deferred(state) => Err(state.waiting_message()),
    }
}

fn finish_operation(
    id: &str,
    result: Result<&Value, &String>,
    guard: task_state::RunGuard,
    policy: InstallPolicy,
) {
    let success = result.is_ok_and(|result| result["hostError"].is_null());
    let mut value = Value::Null;
    mutate(id, |operation| {
        operation.active = false;
        operation.done = success;
        operation.stage = if success { "done" } else { "failed" }.to_string();
        operation.percent = success.then_some(100.0);
        match result {
            Ok(result) => operation.host_error = result["hostError"].as_str().map(str::to_string),
            Err(error) => operation.error = Some(error.clone()),
        }
        operation.result = None;
        value = serde_json::to_value(&*operation).unwrap_or(Value::Null);
    });
    let message = value["error"].as_str().or_else(|| value["hostError"].as_str())
        .unwrap_or("插件安装完成").to_string();
    if !success { logging::log("[Plugin]", &message); }
    let interval = if matches!(policy, InstallPolicy::Required { .. }) {
        super::compatibility::RETRY_INTERVAL_MS
    } else { 60_000 };
    let _ = guard.finish(success, message, Some(value), 0, interval);
}

fn start_asset(
    id: &str,
    action: &str,
    asset: update::PackAsset,
    policy: InstallPolicy,
) -> Result<Operation, String> {
    policy.validate(id, &asset)?;
    crate::server::core::update::assert_downloadable(&asset.url).map_err(|error| error.message)?;
    let guard = claim_operation(id, policy)?;
    let operation = Operation {
        id: id.to_string(),
        action: action.to_string(),
        stage: "downloading".to_string(),
        active: true,
        version: asset.version.clone(),
        required: matches!(policy, InstallPolicy::Required { .. }),
        required_version: matches!(policy, InstallPolicy::Required { .. })
            .then(|| crate::server::core::update::CURRENT_VERSION.to_string()),
        updated_at: logging::now_ms(),
        ..Default::default()
    };
    tasks()
        .lock()
        .map_err(|_| "插件任务状态不可用".to_string())?
        .insert(id.to_string(), operation.clone());
    persist(id);
    let id = id.to_string();
    crate::spawn_task(async move {
        let result = run(&id, &asset, policy).await;
        finish_operation(&id, result.as_ref(), guard, policy);
    });
    Ok(operation)
}

async fn run(
    id: &str,
    asset: &update::PackAsset,
    policy: InstallPolicy,
) -> Result<Value, String> {
    let bytes = update::download(asset, |received, total| {
        mutate(id, |task| {
            task.received = received;
            task.total = total;
            task.percent = if total > 0 {
                Some((received as f64 / total as f64 * 100.0).min(100.0))
            } else {
                None
            };
        });
    })
    .await?;
    mutate(id, |task| {
        task.stage = "verifying".to_string();
        task.percent = None;
    });
    persist(id);
    let expected_id = id.to_string();
    let version = asset.version.clone();
    let require_signed = policy.signed() || asset.require_signed;
    let prepared = tokio::task::spawn_blocking(move || {
        packages::prepare(&bytes, Some(&expected_id), Some(&version), require_signed)
    })
    .await
    .map_err(|error| format!("插件校验任务中断：{error}"))??;
    policy.validate(id, asset)?;
    set_stage(id, "waiting");
    activation::install(prepared, Some((policy, asset.clone()))).await
}

pub async fn start(id: &str, action: &str, target: &str) -> Result<Operation, String> {
    if target != super::host_target() {
        return Err("只能安装当前网关平台的插件包".to_string());
    }
    let packs = super::list_packs();
    let pack = packs.iter().find(|pack| pack.id == id);
    if pack.is_some_and(|pack| pack.update_required) {
        return super::compatibility::start(id, true).await;
    }
    if action == "update" && pack.is_none() {
        return Err("还没有安装这个插件包".to_string());
    }
    let cached = catalog::snapshot();
    let catalog = if cached["checked"] == true && cached["stale"] != true {
        cached
    } else {
        catalog::refresh(true).await?
    };
    let entry = catalog["plugins"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == id))
        .ok_or_else(|| format!("插件索引里没有这个包：{id}"))?;
    if entry["error"] == true {
        return Err(entry["reason"]
            .as_str()
            .unwrap_or("插件更新检查失败")
            .to_string());
    }
    let version = entry["version"]
        .as_str()
        .ok_or_else(|| "当前通道暂无可安装的版本".to_string())?;
    if action == "update"
        && pack.is_some_and(|pack| {
            crate::server::core::update::compare_versions(version, &pack.version) != Some(1)
        })
    {
        return Err("已经是最新版本".to_string());
    }
    let platform = entry["platforms"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["target"] == target))
        .ok_or_else(|| "当前平台暂无安装包".to_string())?;
    start_cached(id, action, target, version, platform, false)
}
