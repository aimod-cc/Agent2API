use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::config::web_tools;
use crate::server::core::{providers, upstream};
use crate::server::errors::{self, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn view() -> Value {
    let providers: Vec<Value> = providers::executable_provider_ids()
        .into_iter()
        .filter_map(|id| {
            let adapter = providers::adapter::adapter_by_id(&id)?;
            let models: Vec<Value> = adapter
                .list_models()
                .into_iter()
                .filter_map(|model| {
                    let profile = upstream::web_tools::Profile::from_model(&model)?;
                    Some(json!({ "id": model["id"], "name": model["name"], "profile": profile }))
                })
                .collect();
            (!models.is_empty())
                .then(|| json!({ "id": id, "label": adapter.label(), "models": models }))
        })
        .collect();
    json!({ "enabled": web_tools::enabled(), "providers": providers })
}

pub async fn get_settings(State(_state): State<ServerState>) -> Response {
    ok_json(view())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    enabled: bool,
}

pub async fn put_settings(State(_state): State<ServerState>, body: Bytes) -> Response {
    let result = parse_body(&body).and_then(|value| {
        serde_json::from_value::<Save>(value)
            .map_err(|_| GatewayError::bad_request("网页工具调用设置需要 enabled 布尔值"))
    });
    let settings = match result {
        Ok(settings) => settings,
        Err(error) => return errors::management_error(error.status_code, error.message),
    };
    match web_tools::save(settings.enabled) {
        Ok(()) => ok_json(view()),
        Err(error) => errors::management_error(error.status_code, error.message),
    }
}
