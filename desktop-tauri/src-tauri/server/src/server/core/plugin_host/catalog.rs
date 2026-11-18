//! 插件商店和更新检查共用官网目录与持久化缓存。

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use futures::{stream, StreamExt};
use tokio::sync::Notify;

use super::{settings, update};
use crate::server::core::task_state::{self, Claim, ManualBackoff};
use crate::server::core::update::UpdateError;
use crate::server::logging;

fn key() -> String {
    key_for(settings::current().channels())
}

fn key_for(channels: crate::server::config::UpdateChannels) -> String {
    format!("pluginCatalog:{}:{}", update::INDEX_SOURCE, channels.key())
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
            task_state::clear_cooldown(&format!("pluginCatalog:{}:{channel}", update::INDEX_SOURCE));
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
            &format!("pluginCatalog:{}:{channel}", update::INDEX_SOURCE),
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
    snapshot_from_state(channels, &state)
}

pub fn snapshot_with_states(states: &HashMap<String, task_state::TaskState>) -> Value {
    let channels = settings::current().channels();
    let default = task_state::TaskState::default();
    let state = states.get(&key_for(channels)).unwrap_or(&default);
    snapshot_from_state(channels, state)
}

fn retain_wasm_platforms(item: &mut Value) {
    if let Some(platforms) = item["platforms"].as_array_mut() {
        let had_assets = !platforms.is_empty();
        platforms.retain(|platform| platform["target"] == super::wasm::manifest::TARGET);
        if had_assets && platforms.is_empty() && item["error"] != true {
            item["reason"] = json!(super::support::LEGACY_REASON);
        }
    }
}

fn snapshot_from_state(channels: crate::server::config::UpdateChannels, state: &task_state::TaskState) -> Value {
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
        for item in items.iter_mut() {
            retain_wasm_platforms(item);
            if let Some(object) = item.as_object_mut() {
                object.remove("official");
                if let Some(Value::String(name)) = object.get_mut("name") {
                    *name = super::pack_view::display_name(name).to_string();
                }
            }
        }
        items.sort_by(|left, right| left["order"].as_u64().unwrap_or(100).cmp(&right["order"].as_u64().unwrap_or(100))
            .then_with(|| left["id"].as_str().cmp(&right["id"].as_str())));
    }
    let stale = state.last_error.is_some() || invalidated;
    result["checked"] = json!(state.last_success_at > 0 && !invalidated);
    result["checkedAt"] = json!(state.last_success_at);
    result["checking"] = json!(state.running());
    result["stale"] = json!(stale);
    result["current"] = json!(super::host_target());
    result["reason"] = json!(state.last_error);
    result["error"] = json!(state.last_error.is_some());
    result["available"] = json!(result["plugins"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    let failure = result["plugins"].as_array().and_then(|items| items.iter().find(|item| item["error"] == true)).cloned();
    if let Some(failure) = failure.filter(|_| !stale) {
        result["error"] = json!(true);
        result["reason"] = failure.get("reason").cloned().unwrap_or(Value::Null);
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
        ManualBackoff::Bypass,
        0,
    )? {
        Claim::Acquired(guard) => guard,
        Claim::Deferred(mut state) => {
            let started = logging::now_ms();
            while state.running() && logging::now_ms() - started < 75_000 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                state = task_state::read(&channel_key)?;
            }
            if state.running() { return Err(update::waiting_message(&state)); }
            if state.last_run_at > 0 { return Ok(snapshot()); }
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
            let partial = value["plugins"].as_array().is_some_and(|items| items.iter().any(|item| item["error"] == true));
            let summary = if partial { "插件列表已刷新，部分版本查询失败" } else { "插件检查完成" };
            guard.finish(true, summary.to_string(), Some(value), 0, interval)?;
            Ok(snapshot())
        }
        Err(error) => {
            let value = task_state::read(&channel_key)?.value.filter(Value::is_object)
                .unwrap_or_else(|| json!({ "plugins": [] }));
            guard.finish(false, error.message.clone(), Some(value), 0, interval)?;
            Err(error.message)
        }
    }
}

async fn fetch_catalog(channels: crate::server::config::UpdateChannels) -> Result<Value, UpdateError> {
    let url = format!("{}/catalog?stable={}&beta={}", update::INDEX_SOURCE, u8::from(channels.stable), u8::from(channels.beta));
    let response = crate::server::core::update::website::fetch_json(&url, 30_000).await?;
    if !response.status.is_success() || response.value["schema"] != 1 {
        return Err(UpdateError::new("官网插件目录暂不可用"));
    }
    let entries = response.value["plugins"].as_array().filter(|items| items.len() <= 200)
        .ok_or_else(|| UpdateError::new("官网插件目录格式无效"))?;
    let cached = snapshot_for(channels);
    let mut icons = HashMap::new();
    let icon_jobs: Vec<_> = entries.iter().filter_map(|entry| {
        let id = entry["id"].as_str()?;
        let icon = entry["icon"].as_str().unwrap_or("");
        let old = cached["plugins"].as_array().and_then(|items| items.iter().find(|item| item["id"] == id));
        (old.is_none_or(|item| item["iconSource"] != entry["icon"] || item["icon"].is_null()))
            .then(|| (id.to_string(), icon.to_string()))
    }).collect();
    let mut fetched = stream::iter(icon_jobs.into_iter().map(|(id, icon)| async move {
        (id, update::read_store_icon(&icon).await)
    })).buffer_unordered(6);
    while let Some((id, icon)) = fetched.next().await { icons.insert(id, json!(icon)); }
    let current = super::host_target();
    let mut plugins = Vec::new();
    for entry in entries {
        let Some(id) = entry["id"].as_str().filter(|id| !id.is_empty() && id.len() <= 64
            && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')) else { continue; };
        let text = |key: &str| entry[key].as_str().unwrap_or("");
        let detail = update::PluginDetail {
            protocol: entry["detail"]["protocol"].as_u64().unwrap_or(0) as u32,
            repo: entry["detail"]["release"]["repo"].as_str().unwrap_or("").to_string(),
            tag_template: entry["detail"]["release"]["tag"].as_str().unwrap_or("v{version}").to_string(),
            asset_template: entry["detail"]["release"]["asset"].as_str().unwrap_or("{id}-{version}-{target}.zip").to_string(),
            homepage: entry["detail"]["homepage"].as_str().unwrap_or("").to_string(),
            require_signed: true,
        };
        let repo = &detail.repo;
        let old = cached["plugins"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == id));
        let icon = old
            .and_then(|item| item.get("icon"))
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| Value::Null);
        let icon = icons.remove(id).unwrap_or(icon);
        let mut value = json!({
            "id": id, "name": super::pack_view::display_name(text("name")), "description": text("description"),
            "order": entry["order"], "category": entry["category"],
            "providers": entry.get("providers").cloned().unwrap_or_else(|| json!([])),
            "repository": repo,
            "homepage": detail.homepage,
            "protocol": detail.protocol, "icon": icon, "iconSource": entry["icon"],
            "requireSigned": true,
            "version": Value::Null, "page": Value::Null, "notes": "", "prerelease": false,
            "platforms": [], "error": false, "reason": Value::Null,
        });
        let release = &entry["release"];
        let selected = if release.is_null() { Ok(None) } else {
            update::pick_release_assets(release, id, Some(&detail))
                .ok_or_else(|| UpdateError::new("官网插件版本或附件格式无效"))
                .and_then(|mut assets| {
                    update::validate_release_urls(release, &assets, id)?;
                    if !channels.accepts(assets.prerelease) { return Err(UpdateError::new("官网插件版本不属于当前通道")); }
                    assets.require_signed = true;
                    Ok(Some(assets))
                })
        };
        match selected {
            Ok(Some(release)) => {
                let compatibility_error = (detail.protocol != agent2api_provider_protocol::PROTOCOL_VERSION)
                    .then(|| format!("插件要求协议 {}，当前网关支持 {}", detail.protocol, agent2api_provider_protocol::PROTOCOL_VERSION));
                let require_signed = value["requireSigned"] == true || release.require_signed;
                value["requireSigned"] = json!(require_signed);
                value["version"] = json!(release.version);
                value["page"] = json!(release.page);
                value["notes"] = json!(release.notes);
                value["publishedAt"] = json!(release.published_at);
                value["prerelease"] = json!(release.prerelease);
                let digests = release.digests.clone();
                value["platforms"] = json!(release.platforms.into_iter().map(|(target, url, size)| {
                    let digest = digests.get(&url).cloned();
                    json!({ "current": target == current || target == super::wasm::manifest::TARGET, "target": target, "url": url, "size": size, "requireSigned": true, "sha256": digest })
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
                value["reason"] = json!(error.message);
                }
        }
        retain_wasm_platforms(&mut value);
        value["checkedAt"] = json!(logging::now_ms());
        plugins.push(value);
    }
    plugins.sort_by(|left, right| left["order"].as_u64().unwrap_or(100).cmp(&right["order"].as_u64().unwrap_or(100))
        .then_with(|| left["id"].as_str().cmp(&right["id"].as_str())));
    Ok(json!({ "plugins": plugins }))
}

pub fn updates() -> Value {
    let catalog = snapshot();
    updates_with_packs(&super::list_packs(), &catalog)
}

pub fn updates_with_packs(packs: &[super::PackSummary], catalog: &Value) -> Value {
    let items = catalog["plugins"].as_array();
    let updates: Vec<Value> = packs.iter().map(|pack| {
        let entry = items.and_then(|items| items.iter().find(|entry| entry["id"] == pack.id));
        let platform = entry.and_then(|entry| entry["platforms"].as_array())
            .and_then(|items| items.iter().find(|item| item["target"] == super::wasm::manifest::TARGET));
        let version = entry.and_then(|item| item["version"].as_str());
        let supported = pack.support_state == super::PackSupportState::Supported;
        let available = supported && version.is_some_and(|version| {
            crate::server::core::update::compare_versions(version, &pack.version) == Some(1)
        }) && platform.is_some();
        let error = entry.and_then(|item| item["error"].as_bool()).unwrap_or(false)
            || catalog["stale"] == true;
        let failure = if catalog["stale"] == true { catalog }
            else { entry.filter(|item| item["error"] == true).unwrap_or(catalog) };
        json!({
            "id": pack.id, "current": pack.version, "latest": version,
            "available": available, "checked": catalog["checked"], "checkedAt": catalog["checkedAt"],
            "notes": entry.map(|entry| &entry["notes"]), "page": entry.map(|entry| &entry["page"]),
            "publishedAt": entry.map(|entry| &entry["publishedAt"]),
            "prerelease": entry.map(|entry| entry["prerelease"] == true).unwrap_or(false),
            "asset": platform, "error": error,
            "reason": if !supported { Some(super::support::LEGACY_REASON.to_string()) }
            else if error {
                failure["reason"].as_str().map(str::to_string)
            } else if entry.is_none() && catalog["checked"] == true {
                Some("插件不在官网目录中，暂无可用更新".to_string())
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
    if !settings.auto_check || !settings.auto_update {
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
        if !settings.auto_check || !settings.auto_update || settings.channels() != channels {
            return;
        }
        let Some(id) = entry["id"].as_str() else {
            continue;
        };
        if entry["error"] == true
            || super::operations::active(id)
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
        let should_update = installed.is_some_and(|pack| {
            pack.support_state == super::PackSupportState::Supported
                && pack.enabled
                && pack.signed
                && entry["version"].as_str().is_some_and(|version| {
                    crate::server::core::update::compare_versions(version, &pack.version) == Some(1)
                })
        });
        if !should_update {
            continue;
        }
        let target = super::host_target();
        let Some(asset) = entry["platforms"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["target"] == super::wasm::manifest::TARGET))
        else {
            continue;
        };
        let version = entry["version"].as_str().unwrap_or("");
        if let Err(error) =
            super::operations::start_cached(id, "update", &target, version, asset, true)
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
