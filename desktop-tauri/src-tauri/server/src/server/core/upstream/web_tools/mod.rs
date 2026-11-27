//! 只消费插件逐模型声明的文本工具契约，不替换原生工具协议。

mod parser;
mod request;
mod stream;
#[cfg(test)]
mod tests;

pub use request::{Context, Prepared};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::config;
use crate::server::errors::GatewayError;

pub(super) const OPEN: &str = "<agent2api_tool_calls>";
pub(super) const CLOSE: &str = "</agent2api_tool_calls>";
pub(super) const MAX_CALLS: usize = 64;
pub(super) const MAX_ARGUMENT_BYTES: usize = 1024 * 1024;
pub(super) const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    protocol: String,
    max_prompt_chars: usize,
}

impl Profile {
    pub fn from_model(model: &Value) -> Option<Self> {
        if crate::server::core::providers::images::model_is_image(model) {
            return None;
        }
        let profile: Self = serde_json::from_value(model.get("toolEmulation")?.clone()).ok()?;
        (profile.protocol == "tagged_json_v1"
            && (1024..=1_000_000).contains(&profile.max_prompt_chars))
        .then_some(profile)
    }
}

pub fn advertise(model: &mut Value) {
    if Profile::from_model(model).is_none() {
        return;
    }
    let enabled = config::web_tools::enabled();
    let Some(fields) = model.as_object_mut() else {
        return;
    };
    fields.insert("supportsToolCall".into(), json!(enabled));
    fields.insert(
        "toolCallingMode".into(),
        json!(if enabled { "emulated" } else { "disabled" }),
    );
    let sources = fields.entry("capSources").or_insert_with(|| json!({}));
    if !sources.is_object() {
        *sources = json!({});
    }
    sources["supportsToolCall"] = json!("gateway");
}

pub fn prepare(body: &Value, models: &[Value]) -> Result<Option<Prepared>, GatewayError> {
    if !config::web_tools::enabled() {
        return Ok(None);
    }
    let wanted = body.get("model").and_then(Value::as_str).unwrap_or("");
    let model = models
        .iter()
        .find(|model| {
            ["id", "name", "upstreamModel"].iter().any(|key| {
                model
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.eq_ignore_ascii_case(wanted))
            })
        })
        .or_else(|| {
            wanted
                .is_empty()
                .then(|| {
                    models
                        .iter()
                        .find(|model| model["isDefault"] == true)
                        .or_else(|| models.first())
                })
                .flatten()
        });
    let Some(profile) = model.and_then(Profile::from_model) else {
        return Ok(None);
    };
    request::prepare(body, &profile)
}

pub fn wrap(
    source: futures::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>>,
    context: Context,
    telemetry: std::sync::Arc<super::usage::RequestTelemetry>,
    failure: std::sync::Arc<std::sync::Mutex<Option<GatewayError>>>,
) -> futures::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>> {
    stream::wrap(source, context, telemetry, failure)
}

pub(super) fn invalid(message: impl Into<String>) -> GatewayError {
    GatewayError::bad_request(message).with_code("web_tools_invalid_request")
}

pub(super) fn failed(message: impl Into<String>) -> GatewayError {
    GatewayError::with_status(502, message).with_code("web_tools_invalid_response")
}
