//! 插件商店和更新检查共用持久化缓存；页面轮询不消耗 GitHub 请求额度。

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Notify;

use super::{settings, update};
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::logging;

fn key() -> String {
    key_for(settings::current().channels())
}

fn key_for(channels: crate::server::config::UpdateChannels) -> String {
    format!("pluginCatalog:{}:{}", update::INDEX_REPO, channels.key())
}

fn signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}

pub fn wake() {
    signal().notify_one();
}

pub fn clear_cooldown() {
    for channel in ["stable", "beta", "both"] {
        let _ =
            task_state::clear_cooldown(&format!("pluginCatalog:{}:{channel}", update::INDEX_REPO));
    }
    wake();
}

pub fn invalidate_channel() {
    let _ = task_state::store_value(&key(), json!({ "plugins": [], "invalidated": true }));
}

pub fn schedule_now() {
    let _ = task_state::schedule_now(&key());
    wake();
}

pub fn reschedule(interval_ms: i64) {
    for channel in ["stable", "beta", "both"] {
        let _ = task_state::reschedule(
            &format!("pluginCatalog:{}:{channel}", update::INDEX_REPO),
            interval_ms,
        );
    }
    wake();
}

pub fn snapshot() -> Value {
    snapshot_for(settings::current().channels())
}

fn snapshot_for(channels: crate::server::config::UpdateChannels) -> Value {
    let state = task_state::read(&key_for(channels)).unwrap_or_default();
    let mut result = state
        .value
        .clone()
        .unwrap_or_else(|| json!({ "plugins": [] }));
    let invalidated = result["invalidated"] == true || result.get("channels") != Some(&json!(channels));
    if invalidated {
        result["plugins"] = json!([]);
    }
    result["channels"] = json!(channels);
    if let Some(items) = result["plugins"].as_array_mut() {
        for item in items {
            if item["error"] == true {
                if let Some(reason) = item["reason"].as_str() {
                    item["reason"] = json!(update::describe_error(reason, 0));
                }
            }
        }
    }
    let stale = state.last_error.is_some() || invalidated;
    result["checked"] = json!(state.last_success_at > 0 && !invalidated);
    result["checkedAt"] = json!(state.last_success_at);
    result["checking"] = json!(state.running());
    result["stale"] = json!(stale);
    result["current"] = json!(super::host_target());
    result["reason"] = json!(state.last_error.as_deref().map(|error| {
        update::describe_error(error, state.retry_at)
    }));
    result["error"] = json!(state.last_error.is_some());
    result["available"] = json!(result["plugins"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    if let Some(error) = result["plugins"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["error"] == true)
                .and_then(|item| item["reason"].as_str())
        })
        .map(str::to_string)
    {
        result["error"] = json!(true);
        result["reason"] = json!(error);
    }
    result
}

pub async fn refresh(manual: bool) -> Result<Value, String> {
    let settings = settings::current();
    let channels = settings.channels();
    let channel_key = key_for(channels);
    let interval = settings.interval * 60_000;
    let guard = match task_state::claim(
        &channel_key,
        interval,
        manual,
        ManualBackoff::Respect,
        60_000,
    )? {
        Claim::Acquired(guard) => guard,
        Claim::Deferred(state) => {
            if state.last_success_at > 0 {
                return Ok(snapshot());
            }
            return Err(update::waiting_message(&state));
        }
    };
    let outcome = fetch_catalog(channels).await;
    let interval = settings::current().interval * 60_000;
    if channel_key != key() {
        drop(guard);
        schedule_now();
        return Ok(snapshot());
    }
    match outcome {
        Ok(mut value) => {
            value["channels"] = json!(channels);
            guard.finish(true, "插件检查完成".to_string(), Some(value), 0, interval)?;
            Ok(snapshot())
        }
        Err(error) => {
            guard.finish(false, error.clone(), None, 0, interval)?;
            Err(error)
        }
    }
}

async fn fetch_catalog(channels: crate::server::config::UpdateChannels) -> Result<Value, String> {
    let entries = update::read_index_result().await?;
    let packs = super::list_packs();
    let cached = snapshot_for(channels);
    let current = super::host_target();
    let mut plugins = Vec::new();
    for entry in entries {
        let detail = update::read_detail(&entry.manifest).await;
        let repo = if entry.repo.is_empty() {
            detail
                .as_ref()
                .map(|detail| detail.repo.as_str())
                .unwrap_or("")
        } else {
            &entry.repo
        };
        let old = cached["plugins"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == entry.id));
        let icon = old
            .and_then(|item| item.get("icon"))
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| Value::Null);
        let icon = if icon.is_null() {
            json!(update::read_store_icon(&entry.icon).await)
        } else {
            icon
        };
        let installed = packs.iter().find(|pack| pack.id == entry.id);
        let mut value = json!({
            "id": entry.id, "name": entry.name, "description": entry.description,
            "official": entry.official, "homepage": detail.as_ref().map(|detail| &detail.homepage),
            "protocol": detail.as_ref().map(|detail| detail.protocol), "icon": icon,
            "requireSigned": entry.require_signed || detail.as_ref().is_some_and(|detail| detail.require_signed),
            "version": Value::Null, "page": Value::Null, "notes": "", "prerelease": false,
            "platforms": [], "error": false, "reason": Value::Null,
        });
        match update::release_for_channels(
            repo,
            &entry.id,
            detail.as_ref(),
            installed.map(|pack| pack.version.as_str()),
            channels,
        )
        .await
        {
            Ok(Some(release)) => {
                let compatibility_error = if installed.is_none() {
                    super::compatibility::version_error(&entry.id, &release.version)
                } else {
                    None
                };
                let require_signed = value["requireSigned"] == true || release.require_signed;
                value["requireSigned"] = json!(require_signed);
                value["version"] = json!(release.version);
                value["page"] = json!(release.page);
                value["notes"] = json!(release.notes);
                value["publishedAt"] = json!(release.published_at);
                value["prerelease"] = json!(release.prerelease);
                value["platforms"] = json!(release.platforms.into_iter().map(|(target, url, size)| {
                    json!({ "current": target == current, "target": target, "url": url, "size": size, "requireSigned": require_signed })
                }).collect::<Vec<_>>());
                if let Some(reason) = compatibility_error {
                    value["error"] = json!(true);
                    value["reason"] = json!(reason);
                }
            }
            Ok(None) => value["reason"] = json!("当前通道暂无可安装的版本"),
            Err(error) => {
                // 失败保留上次发行版，但必须标明不是本轮取得的新结果。
                if let Some(old) = old {
                    for field in [
                        "version",
                        "page",
                        "notes",
                        "publishedAt",
                        "prerelease",
                        "platforms",
                    ] {
                        if let Some(previous) = old.get(field) {
                            value[field] = previous.clone();
                        }
                    }
                }
                value["error"] = json!(true);
                value["reason"] = json!(error);
            }
        }
        plugins.push(value);
    }
    plugins.sort_by(|left, right| {
        right["official"]
            .as_bool()
            .cmp(&left["official"].as_bool())
            .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
    });
    Ok(json!({ "plugins": plugins }))
}

pub fn updates() -> Value {
    let catalog = snapshot();
    let items = catalog["plugins"].as_array();
    let updates: Vec<Value> = super::list_packs().into_iter().map(|pack| {
        if let Some(required) = super::compatibility::update_for(&pack) {
            return required;
        }
        let entry = items.and_then(|items| items.iter().find(|entry| entry["id"] == pack.id));
        let target = super::host_target();
        let platform = entry.and_then(|entry| entry["platforms"].as_array())
            .and_then(|items| items.iter().find(|item| item["target"] == target));
        let version = entry.and_then(|item| item["version"].as_str());
        let available = version.is_some_and(|version| {
            crate::server::core::update::compare_versions(version, &pack.version) == Some(1)
        }) && platform.is_some();
        let error = entry.and_then(|item| item["error"].as_bool()).unwrap_or(false)
            || catalog["stale"] == true;
        json!({
            "id": pack.id, "current": pack.version, "latest": version,
            "available": available, "checked": catalog["checked"], "checkedAt": catalog["checkedAt"],
            "notes": entry.map(|entry| &entry["notes"]), "page": entry.map(|entry| &entry["page"]),
            "publishedAt": entry.map(|entry| &entry["publishedAt"]),
            "prerelease": entry.map(|entry| entry["prerelease"] == true).unwrap_or(false),
            "asset": platform, "error": error,
            "reason": if error {
                entry.and_then(|entry| entry["reason"].as_str()).map(str::to_string)
                    .or_else(|| catalog["reason"].as_str().map(str::to_string))
            } else if entry.is_none() && catalog["checked"] == true {
                Some("插件不在当前索引中，请从原来源手动更新".to_string())
            } else if available { None }
            else if catalog["checked"] != true { Some("尚未检查更新".to_string()) }
            else if version.is_none() || platform.is_none() { Some("当前通道暂无可安装的版本".to_string()) }
            else { Some("已经是最新版本".to_string()) },
        })
    }).collect();
    json!({ "updates": updates, "checkedAt": catalog["checkedAt"],
        "checking": catalog["checking"], "error": catalog["error"], "reason": catalog["reason"] })
}

async fn apply_automatic() {
    let settings = settings::current();
    if !settings.auto_check || (!settings.auto_update && !settings.auto_install_official) {
        return;
    }
    let channels = settings.channels();
    let catalog = snapshot();
    if catalog["stale"] == true || catalog["checked"] != true {
        return;
    }
    let Some(items) = catalog["plugins"].as_array() else {
        return;
    };
    for entry in items {
        let settings = settings::current();
        if !settings.auto_check || settings.channels() != channels {
            return;
        }
        let Some(id) = entry["id"].as_str() else {
            continue;
        };
        if entry["error"] == true
            || super::operations::active(id)
            || entry["official"] != true
            || entry["version"].as_str().is_none()
        {
            continue;
        }
        if entry["protocol"].as_u64().is_some_and(|protocol| {
            protocol != agent2api_provider_protocol::PROTOCOL_VERSION as u64
        }) {
            continue;
        }
        let packs = super::list_packs();
        let installed = packs.iter().find(|pack| pack.id == id);
        let should_install = match installed {
            Some(pack) => {
                settings.auto_update
                    && !pack.update_required
                    && pack.enabled
                    && pack.signed
                    && entry["version"].as_str().is_some_and(|version| {
                        crate::server::core::update::compare_versions(version, &pack.version)
                            == Some(1)
                    })
            }
            None => settings.auto_install_official,
        };
        if !should_install {
            continue;
        }
        let target = super::host_target();
        let Some(asset) = entry["platforms"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["target"] == target))
        else {
            continue;
        };
        let version = entry["version"].as_str().unwrap_or("");
        let action = if installed.is_some() {
            "update"
        } else {
            "install"
        };
        if let Err(error) =
            super::operations::start_cached(id, action, &target, version, asset, true)
        {
            logging::verbose("[Plugin]", &format!("自动处理 {id}：{error}"));
            continue;
        }
    }
}

pub fn spawn() {
    crate::spawn_task(async move {
        reschedule(settings::current().interval * 60_000);
        loop {
            super::compatibility::apply().await;
            let settings = settings::current();
            let state = task_state::read(&key()).unwrap_or_default();
            let network_enabled = settings.auto_check;
            if network_enabled && !state.running() && logging::now_ms() >= state.due_at() {
                if let Err(error) = refresh(false).await {
                    logging::verbose("[Plugin]", &format!("插件自动检查：{error}"));
                }
            }
            if network_enabled {
                apply_automatic().await;
            }
            tokio::select! {
                _ = signal().notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(10)) => {},
            }
        }
    });
}
