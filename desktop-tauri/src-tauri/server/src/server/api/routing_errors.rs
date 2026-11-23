use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::error_policy::{self, ErrorSettings};
use crate::server::errors::{management_error, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

pub async fn get(State(state): State<ServerState>) -> Response {
    ok_json(view(&state, &error_policy::snapshot()))
}

pub async fn put(State(state): State<ServerState>, body: Bytes) -> Response {
    let result = (|| {
        let payload = parse_body(&body)?;
        let settings = payload.get("settings").cloned().ok_or_else(|| GatewayError::bad_request("缺少错误处理设置"))?;
        let settings: ErrorSettings = serde_json::from_value(settings).map_err(|_| GatewayError::bad_request("错误处理设置格式不正确"))?;
        let revision = payload["revision"].as_str().ok_or_else(|| GatewayError::bad_request("缺少设置版本，请重新读取"))?;
        error_policy::save(settings, revision)
    })();
    match result {
        Ok(snapshot) => ok_json(view(&state, &snapshot)),
        Err(error) => management_error(error.status_code, error.message),
    }
}

fn view(_state: &ServerState, snapshot: &error_policy::ErrorPolicySnapshot) -> Value {
    let mut providers = crate::server::core::providers::summary_json(|_| 0, &[]);
    for item in crate::server::core::custom_providers::list() {
        if let Some(id) = item["id"].as_str() {
            providers.push(json!({ "id": id, "label": item["name"].as_str().unwrap_or(id) }));
        }
    }
    let provider_rules = crate::server::core::plugin_host::error_rules::all_provider_rules();
    json!({ "settings": snapshot.settings, "defaults": ErrorSettings::default(),
        "revision": error_policy::revision(snapshot), "configured": snapshot.configured, "legacy": snapshot.legacy,
        "providers": providers.into_iter().map(|item| json!({ "id": item["id"], "label": item["label"] })).collect::<Vec<_>>(),
        "providerRules": provider_rules })
}
