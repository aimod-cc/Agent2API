//! 本机客户端的原生配置管理。保存的方案与磁盘上的实际配置分别读取。

pub mod apply;
mod backups;
pub mod catalog;
mod clients;
pub mod configurations;
pub mod configuration_actions;
pub mod discover;
mod dsh_installation;
pub mod dsh_paths;
mod dsh_patch;
mod dsh_document;
#[cfg(windows)]
mod dsh_windows;
pub mod files;
mod json_edit;
pub mod model_config;
pub mod native;
pub mod paths;
pub mod preview;
pub mod quick_connect;
mod quick_profiles;
pub mod registry;
pub mod resources;
pub mod store;
mod toml_editing;
pub mod types;
pub mod validate;
mod yaml_editing;

pub use registry::{find, ClientDef, ConfigFormat, CLIENTS};
pub use types::{active_profile, Model, Profile};

use serde_json::{json, Value};

pub fn snapshot(port: u16) -> Value {
    snapshot_with_options(port, false)
}

pub fn snapshot_with_options(port: u16, force_refresh: bool) -> Value {
    if force_refresh {
        paths::invalidate_caches();
        dsh_installation::invalidate_cache();
    }
    let gateway = format!("http://127.0.0.1:{port}");
    let current = crate::server::config::current();
    let default_model = current.default_model();
    let mut keys = crate::server::core::api_keys::entries_from(&current.raw())
        .into_iter().filter(crate::server::core::api_keys::ApiKeyEntry::is_usable)
        .map(|key| json!({ "id": key.id, "name": key.name, "allowedModels": key.allowed_models, "allowedProviders": key.allowed_providers }))
        .collect::<Vec<_>>();
    if crate::server::config::parse::env_api_key_value().is_some() {
        keys.push(json!({ "id": "env", "name": "环境变量 Key", "allowedModels": [], "allowedProviders": [] }));
    }
    let saved = store::load_snapshot();
    let preferences = paths::load_preferences();
    let binaries = paths::binary_index(!force_refresh);
    let backup_ids = backups::scan_directory().ok();
    let clients = std::thread::scope(|scope| {
        let handles = CLIENTS
            .iter()
            .map(|def| {
                let gateway = gateway.as_str();
                let saved = &saved;
                let preferences = &preferences;
                let binaries = &binaries;
                let backup_ids = backup_ids.as_deref();
                scope.spawn(move || {
                    client_view_with(
                        def,
                        gateway,
                        saved,
                        preferences,
                        binaries,
                        backup_ids,
                        force_refresh,
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .zip(CLIENTS.iter())
            .map(|(handle, def)| {
                handle.join().unwrap_or_else(|_| {
                    json!({ "id": def.id, "name": def.name, "readError": "检测客户端失败" })
                })
            })
            .collect::<Vec<_>>()
    });
    json!({
        "clients": clients,
        "gateway": {
            "baseUrl": gateway,
            "defaultModel": default_model,
            "requiresKey": true,
            "keys": keys,
        },
        "detectedAt": crate::server::logging::now_ms(),
        "host": { "name": std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default() },
    })
}

pub fn warm_up() {
    let binaries = paths::binary_index(true);
    let dsh_bin = find("dsh").and_then(|def| binaries.find(def));
    let _ = dsh_installation::desktop();
    if dsh_bin.is_some() {
        let _ = dsh_installation::cli(dsh_bin.as_deref());
    }
}

fn client_view_with(
    def: &ClientDef,
    gateway: &str,
    saved_snapshot: &Result<Value, String>,
    preferences: &Result<paths::Preferences, String>,
    binaries: &paths::BinaryIndex,
    backup_ids: Option<&[String]>,
    allow_spawn: bool,
) -> Value {
    let detection = paths::detect_with_context(def, preferences, binaries, allow_spawn);
    let (mut live, read_error) = discover::read_live(def, &detection.path);
    for profile in &mut live {
        profile.kind = kind_of(&profile.base_url, gateway).into();
    }
    let saved = match saved_snapshot {
        Ok(value) => store::load_from_snapshot(value, def.id),
        Err(error) => Err(error.clone()),
    };
    let state = if read_error.is_some() {
        "read_error"
    } else if !detection.detected {
        "missing"
    } else if !detection.path.is_file() {
        "uninitialized"
    } else if detection.unsupported || !def.writable {
        "unsupported"
    } else {
        "ready"
    };
    let writable = def.writable && !detection.unsupported && read_error.is_none();
    let (mut profiles, revision) = saved.as_ref().cloned().unwrap_or_default();
    for profile in &mut profiles { model_config::normalize(def, profile); }
    let applied = apply::applied_status_with_ids(def.id, &detection.path, backup_ids).unwrap_or_else(
        |error| json!({ "canUndo": false, "canDisconnect": false, "error": error }),
    );
    let views = profiles
        .iter()
        .map(|profile| {
            let mut view = profile.view();
            let in_file = live
                .iter()
                .any(|current| clients::equivalent(def, &detection.path, profile, current));
            if let Some(object) = view.as_object_mut() {
                object.insert("inFile".into(), json!(in_file));
                object.insert(
                    "validationError".into(),
                    json!(validate::profile(def, profile).err()),
                );
            }
            view
        })
        .collect::<Vec<_>>();
    let mut view = json!({
        "id": def.id, "name": def.name, "vendor": def.vendor,
        "hue": def.hue, "letter": def.letter,
        "icon": format!("assets/agents/{}", def.icon), "iconFull": def.icon_full,
        "configPath": detection.path.display().to_string(),
        "resolvedPath": detection.path.display().to_string(),
        "pathOverride": detection.override_path,
        "format": def.format.as_str(), "configMode": def.behavior,
        "protocol": def.protocols.first().copied().unwrap_or(""), "protocols": def.protocols,
        "installed": detection.detected, "detected": detection.detected,
        "detectionState": state, "evidence": detection.evidence,
        "version": detection.version, "writable": writable,
        "supportsModels": def.models, "supportsEffort": def.effort,
        "supportsDefault": def.default_model, "supportsToggle": def.toggle,
        "supportsGateway": def.gateway, "supportsEnvKey": def.env_key,
        "supportNote": detection.note.as_deref().unwrap_or(def.note),
        "docsUrl": def.docs, "needsRestart": def.needs_restart,
        "resources": resources::capabilities_for(def, &detection),
        "configurations": configurations::list(def, &detection.path, &profiles, &live, writable).iter().map(configurations::Configuration::view).collect::<Vec<_>>(),
        "profiles": views, "live": live.iter().map(|profile| {
            let mut view = profile.view();
            view["editable"] = json!(writable && native::editable(def, profile));
            view["deletable"] = json!(writable);
            view
        }).collect::<Vec<_>>(),
        "nativeRevision": native::revision(&live),
        "profilesAreLive": false, "readError": read_error,
        "storeError": saved.err(), "revision": revision,
        "viaGateway": live.iter().any(|p| p.enabled && is_gateway(&p.base_url, gateway)),
        "activeId": active_profile(&live).map(|p| &p.id), "applied": applied,
    });
    view["modelFields"] = model_config::declared_fields(def, &detection.path);
    view["supportsEffort"] = json!(def.effort && !(def.id == "kimi-code" && paths::kimi_legacy(&detection.path)));
    view["quickProfiles"] = json!(quick_profiles::profiles(def, &live, gateway).iter().map(Profile::view).collect::<Vec<_>>());
    view["configTargets"] = json!(detection.config_targets);
    view["configLayout"] = json!(if def.id == "dsh" { if dsh_paths::is_patch(&detection.path) { "profile" } else { "legacy" } } else { "" });
    view
}

pub fn is_gateway(base: &str, gateway: &str) -> bool {
    let (Ok(base), Ok(gateway)) = (url::Url::parse(base), url::Url::parse(gateway)) else {
        return false;
    };
    let local = |url: &url::Url| {
        matches!(
            url.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
    };
    base.scheme() == gateway.scheme()
        && (base.host_str() == gateway.host_str() || local(&base) && local(&gateway))
        && base.port_or_known_default() == gateway.port_or_known_default()
}

pub fn kind_of(base: &str, gateway: &str) -> &'static str {
    if is_gateway(base, gateway) {
        return "gateway";
    }
    let Ok(url) = url::Url::parse(base) else {
        return "official";
    };
    match url.host_str().unwrap_or("") {
        "localhost" | "127.0.0.1" | "[::1]" | "::1" => "local",
        "api.anthropic.com"
        | "api.openai.com"
        | "generativelanguage.googleapis.com"
        | "api.deepseek.com"
        | "open.bigmodel.cn"
        | "api.moonshot.cn"
        | "api.moonshot.ai" => "official",
        _ => "relay",
    }
}
