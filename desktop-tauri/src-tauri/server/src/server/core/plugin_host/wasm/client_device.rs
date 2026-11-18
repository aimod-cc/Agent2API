use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{context::Context, desktop_credentials as files, native};

pub(super) fn helper(provider: &str) -> Option<&'static str> {
    match provider {
        "qwenwork" => Some("desktop.device.qwenwork.v1"),
        "qwenwork-intl" => Some("desktop.device.qwenwork-intl.v1"),
        "qoder" => Some("desktop.device.qoder.v1"),
        "qoder-intl" => Some("desktop.device.qoder-intl.v1"),
        _ => None,
    }
}

pub(super) fn candidate(provider: &str, id: &str) -> Option<Value> {
    if helper(provider) != Some(id) { return None; }
    Some(json!({ "pack": "agent2api-host", "version": "client-device-v1", "provider": provider,
        "id": id, "operations": ["read"], "digest": "client-device-v1" }))
}

pub(super) async fn invoke(context: &Context, params: &Value) -> Result<Value, proto::WireError> {
    let provider = &context.shared.options.provider;
    let id = helper(provider).ok_or_else(files::invalid)?;
    if params["helper"] != id || params["operation"] != "read"
        || params["payload"].as_object().is_none_or(|fields| !fields.is_empty())
        || !context.shared.options.manifest.helpers.iter().any(|value| value == id) {
        return Err(files::invalid());
    }
    native::authorized_builtin(provider, id)?;
    let provider = provider.clone();
    let result = crate::server::core::blocking::run(move || {
        let home = files::home()?;
        let candidates = match provider.as_str() {
            "qwenwork" => vec![home.join(".qoderworkcn/.auth/machine_id"), home.join(".qoderworkcn/machine_id")],
            "qwenwork-intl" => vec![home.join(".qoderwork/.auth/machine_id"), home.join(".qoderwork/machine_id")],
            "qoder" | "qoder-intl" => vec![home.join(".qoder-proxy/machine_id"), home.join(".qoder/.auth/machine_id"), home.join(".qoder/machine_id")],
            _ => return Err(files::invalid()),
        };
        for path in candidates {
            if !path.exists() { continue; }
            let bytes = files::file(&path, 1024)?;
            let value = std::str::from_utf8(&bytes).map_err(|_| files::invalid())?.trim();
            if value.is_empty() || value.len() > 256 || value.contains('|')
                || value.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
                return Err(files::invalid());
            }
            return Ok(json!({ "machineId": value, "available": true }));
        }
        Ok(json!({ "machineId": null, "available": false }))
    }).await.map_err(|_| files::invalid())?;
    native::authorized_builtin(&context.shared.options.provider, id)?;
    if context.shared.cancel.is_cancelled() { return Err(files::invalid()); }
    result
}
