pub(crate) mod declaration;
pub(super) mod binding;
mod lock;
pub(crate) mod operations;
mod paths;
pub(crate) mod recovery;
mod secret;
mod storage;
mod windows;
pub(crate) use secret::Secret;

pub(crate) use operations::{cancel, confirm, prepare, retry_start, status, restore, settings, configure};

use serde_json::{json, Value};
use super::manifest::Options;

pub(crate) fn validate_metadata(options: &Options, value: &Value) -> Result<(), String> {
    let metadata = &value[0]["desktop_launch"];
    match &options.manifest.desktop_launch {
        Some(declaration) if *metadata == declaration.summary() => Ok(()),
        None if metadata.is_null() => Ok(()),
        _ => Err("插件本机启动声明与权限清单不一致".into()),
    }
}

pub(crate) fn summaries() -> Value {
    let mut result = serde_json::Map::new();
    for provider in super::super::installed_ids() {
        if !super::super::serves_provider(&provider) { continue; }
        let Some(host) = super::super::host_for_provider(&provider).filter(|host| host.ready()) else { continue; };
        let Some(declaration) = host.options.wasm.as_ref().and_then(|options| options.manifest.desktop_launch.as_ref()) else { continue; };
        result.insert(provider, declaration.summary());
    }
    Value::Object(result)
}

pub(crate) fn contract() -> Value {
    json!({ "capability": declaration::CAPABILITY, "entry": "local_desktop_confirmation",
        "methods": [declaration::CHECK, declaration::PREPARE], "storage": ["json_file", "sqlite", "keyring"],
        "platforms": ["windows"], "implicitExecution": false, "credentialsReturnedToUi": false })
}

pub(crate) fn wipe(value: &mut Value) {
    use zeroize::Zeroize;
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(wipe),
        Value::Object(fields) => fields.values_mut().for_each(wipe),
        _ => { *value = Value::Null; },
    }
}

pub(super) fn close_for_host(host: &super::super::Host) { operations::close_for_host(host); }
pub(super) async fn wait_for_host(host: &super::super::Host) { operations::wait_for_host(host).await; }
pub(super) fn release_ready(host: &super::super::Host) -> bool { operations::release_ready(host) }
