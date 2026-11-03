//! 官方配套插件的最低版本校验独立于日常更新偏好。

use serde_json::{json, Value};

use super::install::{InstalledPack, PackRegistry};
use super::{operations, update, PackSummary};
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::core::update::{compare_versions, CURRENT_VERSION};
use crate::server::logging;

pub(super) const RETRY_INTERVAL_MS: i64 = 5 * 60_000;
const OFFICIAL_PACKS: &[&str] = &["agent2api-providers", "agent2api-web-providers"];

pub(super) fn minimum_version(id: &str) -> Option<&'static str> {
    OFFICIAL_PACKS.contains(&id).then_some(CURRENT_VERSION)
}

pub(super) fn version_error(id: &str, version: &str) -> Option<String> {
    let minimum = minimum_version(id)?;
    if matches!(compare_versions(version, minimum), Some(0 | 1)) {
        return None;
    }
    Some(format!(
        "插件「{id}」v{version} 低于当前网关要求的 v{minimum}，补齐前暂不加载；请在插件页查看进度或重试"
    ))
}

pub(crate) fn provider_error(provider: &str) -> Option<String> {
    let registry = PackRegistry::load();
    let owner = super::ownership::owners(&registry);
    let id = owner.get(provider)?;
    let pack = registry.packs.iter().find(|pack| &pack.id == id)?;
    version_error(&pack.id, &pack.version)
}

fn key(id: &str) -> String {
    format!("pluginRequirement:{id}:{CURRENT_VERSION}:{}", super::host_target())
}

fn cached_asset(state: &task_state::TaskState, id: &str) -> Option<update::PackAsset> {
    let asset: update::PackAsset = serde_json::from_value(state.value.as_ref()?.clone()).ok()?;
    let minimum = minimum_version(id)?;
    (matches!(compare_versions(&asset.version, minimum), Some(0 | 1))
        && asset.name == update::asset_name(id, &asset.version, &super::host_target())
        && asset.require_signed)
        .then_some(asset)
}

pub(super) fn update_for(pack: &PackSummary) -> Option<Value> {
    if !pack.update_required {
        return None;
    }
    let state = task_state::read(&key(&pack.id)).unwrap_or_default();
    let asset = cached_asset(&state, &pack.id);
    let task = operations::snapshot()["operations"].as_array()
        .and_then(|tasks| tasks.iter().find(|task| task["id"] == pack.id
            && task["requiredVersion"].as_str() == Some(CURRENT_VERSION)))
        .cloned()
        .unwrap_or(Value::Null);
    let operation_error = task["error"].as_str().or_else(|| task["hostError"].as_str());
    let error = state.last_error.as_deref().or(operation_error);
    let operation_state = task_state::read(&format!("pluginOperation:{}", pack.id)).unwrap_or_default();
    let reason = error.map(|error| update::describe_error(error, state.retry_at)).unwrap_or_else(|| {
        if pack.signed {
            "正在自动补齐当前网关所需的官方插件；日常更新开关不影响本次修复".to_string()
        } else {
            "未签名包不会被自动覆盖，请点击补齐插件或从文件安装符合最低版本要求的包".to_string()
        }
    });
    Some(json!({
        "id": pack.id, "current": pack.version, "latest": asset.as_ref().map(|asset| &asset.version),
        "required": true, "requiredVersion": pack.required_version,
        "repairing": state.running() || operations::active(&pack.id),
        "available": asset.is_some(), "checked": state.last_success_at > 0,
        "checkedAt": state.last_success_at, "retryAt": state.retry_at.max(operation_state.retry_at),
        "asset": asset.as_ref().map(|asset| json!({
            "target": super::host_target(), "url": asset.url, "size": asset.size,
            "current": true, "requireSigned": true,
        })),
        "page": asset.as_ref().map(|asset| &asset.page),
        "prerelease": asset.as_ref().is_some_and(|asset| {
            crate::server::core::update::is_prerelease_version(&asset.version)
        }),
        "reason": reason, "error": error.is_some(),
    }))
}

async fn find_asset(id: &str) -> Result<update::PackAsset, String> {
    let mut detail = update::PluginDetail {
        protocol: agent2api_provider_protocol::PROTOCOL_VERSION,
        repo: update::FALLBACK_REPO.to_string(),
        tag_template: if id == "agent2api-web-providers" { "web-v{version}" } else { "v{version}" }.to_string(),
        alt_tag_templates: Vec::new(),
        asset_template: "{id}-{version}-{target}.zip".to_string(),
        homepage: String::new(),
        require_signed: true,
    };
    match update::read_index_result().await {
        Ok(entries) => {
            if let Some(entry) = entries.into_iter().find(|entry| entry.id == id) {
                if !entry.official {
                    return Err(format!("插件「{id}」在当前索引中不是官方包，未自动替换"));
                }
                if let Some(remote) = update::read_detail(&entry.manifest).await {
                    detail = remote;
                }
                if !entry.repo.is_empty() {
                    detail.repo = entry.repo;
                }
            }
        }
        Err(error) => logging::verbose("[Plugin]", &format!("插件索引读取失败，尝试内置官方配套源：{error}")),
    }
    let minimum = minimum_version(id).ok_or_else(|| "这个插件不属于官方配套包".to_string())?;
    update::required_release(&detail.repo, id, &detail, minimum, &super::host_target()).await
}

async fn resolve(id: &str, manual: bool) -> Result<update::PackAsset, String> {
    let state = task_state::read(&key(id))?;
    if state.last_error.is_none()
        && logging::now_ms().saturating_sub(state.last_success_at) < RETRY_INTERVAL_MS
    {
        if let Some(asset) = cached_asset(&state, id) {
            return Ok(asset);
        }
    }
    let guard = match task_state::claim(
        &key(id), RETRY_INTERVAL_MS, manual, ManualBackoff::Respect, 60_000,
    )? {
        Claim::Acquired(guard) => guard,
        Claim::Deferred(state) => return Err(state.last_error.as_deref()
            .map(|error| update::describe_error(error, state.retry_at))
            .unwrap_or_else(|| update::waiting_message(&state))),
    };
    match find_asset(id).await {
        Ok(asset) => {
            guard.finish(
                true, format!("找到官方配套插件 v{}", asset.version),
                Some(json!(asset)), 0, RETRY_INTERVAL_MS,
            )?;
            Ok(asset)
        }
        Err(error) => {
            guard.finish(false, error.clone(), None, 0, RETRY_INTERVAL_MS)?;
            Err(error)
        }
    }
}

fn installed(id: &str, automatic: bool) -> Result<InstalledPack, String> {
    let pack = PackRegistry::load().packs.into_iter().find(|pack| pack.id == id)
        .ok_or_else(|| "还没有安装这个插件包".to_string())?;
    if version_error(id, &pack.version).is_none() {
        return Err("插件已满足当前网关的最低版本要求，不需要补齐".to_string());
    }
    if automatic && !pack.signed {
        return Err("未签名包需要用户手动确认更新，未自动覆盖".to_string());
    }
    Ok(pack)
}

pub(super) async fn start(id: &str, manual: bool) -> Result<operations::Operation, String> {
    installed(id, !manual)?;
    if operations::active(id) {
        return Err("这个插件正在下载或安装，请稍候".to_string());
    }
    let operation_state = task_state::read(&format!("pluginOperation:{id}"))?;
    if operation_state.running() || operation_state.retry_at > logging::now_ms() {
        return Err(operation_state.waiting_message());
    }
    let asset = resolve(id, manual).await?;
    installed(id, !manual)?;
    operations::start_required(id, asset, !manual)
}

pub(super) async fn apply() {
    let Ok(states) = task_state::read_all() else { return };
    let now = logging::now_ms();
    let packs = PackRegistry::load();
    for pack in packs.packs {
        if !pack.signed || version_error(&pack.id, &pack.version).is_none()
            || operations::active(&pack.id)
            || states.get(&key(&pack.id)).is_some_and(|state| state.running() || now < state.due_at())
            || states.get(&format!("pluginOperation:{}", pack.id))
                .is_some_and(|state| state.running() || now < state.retry_at)
        {
            continue;
        }
        if let Err(error) = start(&pack.id, false).await {
            logging::verbose("[Plugin]", &format!("最低版本补齐 {}：{error}", pack.id));
            continue;
        }
        while operations::active(&pack.id) {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

pub(super) fn clear_cooldown() {
    for id in OFFICIAL_PACKS {
        let _ = task_state::clear_cooldown(&key(id));
        let _ = task_state::store_value(&key(id), Value::Null);
        let _ = task_state::schedule_now(&key(id));
        let _ = task_state::clear_cooldown(&format!("pluginOperation:{id}"));
        let _ = task_state::schedule_now(&format!("pluginOperation:{id}"));
    }
    super::catalog::wake();
}
