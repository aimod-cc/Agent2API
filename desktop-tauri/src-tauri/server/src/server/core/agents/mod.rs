//! 本机客户端的原生配置管理。保存的方案与磁盘上的实际配置分别读取。

pub mod apply;
mod backups;
mod clients;
pub mod discover;
pub mod files;
mod json_edit;
pub mod native;
pub mod paths;
pub mod preview;
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
    let gateway = format!("http://127.0.0.1:{port}");
    let mut keys = crate::server::core::api_keys::entries_from(&crate::server::config::current().raw())
        .into_iter().filter(crate::server::core::api_keys::ApiKeyEntry::is_usable)
        .map(|key| json!({ "id": key.id, "name": key.name, "allowedModels": key.allowed_models, "allowedProviders": key.allowed_providers }))
        .collect::<Vec<_>>();
    if crate::server::config::parse::env_api_key_value().is_some() {
        keys.push(json!({ "id": "env", "name": "环境变量 Key", "allowedModels": [], "allowedProviders": [] }));
    }
    json!({
        "clients": CLIENTS.iter().map(|def| client_view(def, &gateway)).collect::<Vec<_>>(),
        "gateway": {
            "baseUrl": gateway,
            "defaultModel": crate::server::config::current().default_model(),
            "requiresKey": true,
            "keys": keys,
        },
        "detectedAt": crate::server::logging::now_ms(),
        "host": { "name": std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default() },
    })
}

fn client_view(def: &ClientDef, gateway: &str) -> Value {
    let detection = paths::detect(def);
    let (mut live, read_error) = discover::read_live(def, &detection.path);
    for profile in &mut live {
        profile.kind = kind_of(&profile.base_url, gateway).into();
    }
    let saved = store::load_with_revision(def.id);
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
    let (profiles, revision) = saved.as_ref().cloned().unwrap_or_default();
    let applied = apply::applied_status(def.id, &detection.path).unwrap_or_else(
        |error| json!({ "canUndo": false, "canDisconnect": false, "error": error }),
    );
    let views = profiles
        .iter()
        .map(|profile| {
            let mut view = profile.view();
            let in_file = live
                .iter()
                .any(|current| clients::equivalent(def, profile, current));
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
    json!({
        "id": def.id, "name": def.name, "vendor": def.vendor,
        "hue": def.hue, "letter": def.letter,
        "icon": format!("assets/agents/{}", def.icon), "iconFull": def.icon_full,
        "configPath": detection.path.display().to_string(),
        "resolvedPath": detection.path.display().to_string(),
        "pathOverride": paths::path_override(def.id).unwrap_or_default(),
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
        "resources": resources::capabilities(def),
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
    })
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
