use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::config::vision_compat::{self, Mode, ModelSource, VisionSettings, VisionTarget};
use crate::server::core::{capability, providers::catalog, routing_groups};
use crate::server::errors::{self, GatewayError};
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

fn shared_target() -> Option<VisionTarget> { vision_compat::shared_target() }

fn view(state: &ServerState) -> Result<Value, GatewayError> {
    let snapshot = vision_compat::current();
    let settings = snapshot.settings.as_ref().map_err(|error| GatewayError::new(error.clone()))?;
    let mut choices = routing_groups::choices(&state.store());
    for choice in &mut choices {
        let provider = choice["provider"].as_str().unwrap_or("");
        let model = choice["model"].as_str().unwrap_or("");
        let images = routing_groups::catalog_model(provider, model)
            .and_then(|item| capability::images_for_channel(provider, &item));
        choice["capabilities"]["supportsImages"] = json!(images);
    }
    Ok(json!({
        "config": settings, "revision": snapshot.revision,
        "models": choices, "sharedTarget": shared_target(),
        "maxTargets": vision_compat::MAX_TARGETS, "maxOverrides": vision_compat::MAX_OVERRIDES,
    }))
}

fn response(result: Result<Value, GatewayError>) -> Response {
    match result {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(error.status_code, error.message),
    }
}

pub async fn get_settings(State(state): State<ServerState>) -> Response {
    match tokio::task::spawn_blocking(move || view(&state)).await {
        Ok(result) => response(result),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveSettings {
    config: VisionSettings,
    revision: String,
    #[serde(default)]
    consent: bool,
}

fn validate_available(target: &VisionTarget) -> Result<(), GatewayError> {
    let item = routing_groups::catalog_model(&target.provider, &target.model)
        .ok_or_else(|| GatewayError::bad_request("所选视觉模型已不在提供商目录中，请重新选择"))?;
    if capability::images_for_channel(&target.provider, &item) != Some(true) {
        return Err(GatewayError::bad_request("请选择已确认支持图片的视觉模型；能力未声明时请先在模型管理中确认"));
    }
    if !catalog::forwarding_providers(&target.model).contains(&target.provider) {
        return Err(GatewayError::bad_request("所选视觉模型尚未启用，请先在模型管理中启用"));
    }
    Ok(())
}

pub async fn put_settings(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body).and_then(|value| serde_json::from_value::<SaveSettings>(value)
        .map_err(|error| GatewayError::bad_request(format!("图片兼容设置不完整：{error}")))) {
        Ok(payload) => payload,
        Err(error) => return response(Err(error)),
    };
    match tokio::task::spawn_blocking(move || {
        vision_compat::validate(&payload.config).map_err(GatewayError::bad_request)?;
        let previous = vision_compat::current();
        let changed = previous.settings.as_ref().map_or(true, |old| old != &payload.config);
        if payload.config.enabled && changed && !payload.consent {
            return Err(GatewayError::bad_request("请先确认图片与相关对话将交给所选视觉模型处理，并可能增加等待时间和费用"));
        }
        if payload.config.enabled {
            if payload.config.uses(Mode::Assist) {
                let target = match payload.config.assist.source {
                    ModelSource::Shared => {
                        let current = shared_target();
                        if payload.config.assist.shared_target != current {
                            return Err(GatewayError::with_status(409, "视觉分析模型已在其他位置更改，请刷新后重新确认图片处理授权"));
                        }
                        current
                    },
                    ModelSource::Custom => payload.config.assist.target.clone(),
                }.ok_or_else(|| GatewayError::bad_request("请先配置辅助识图的视觉模型"))?;
                validate_available(&target)?;
            }
            if payload.config.uses(Mode::Reroute) && payload.config.reroute.fallback == vision_compat::Fallback::Designated {
                for target in &payload.config.reroute.targets { validate_available(target)?; }
            }
        }
        vision_compat::save(payload.config, &payload.revision)?;
        view(&state)
    }).await {
        Ok(result) => response(result),
        Err(error) => errors::management_error(500, error.to_string()),
    }
}
