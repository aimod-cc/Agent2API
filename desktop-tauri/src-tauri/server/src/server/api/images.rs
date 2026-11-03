//! OpenAI Images 生成与编辑入口；鉴权、模型绑定、账号重试与记账复用网关主链。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::key_scope::{self, KeyScope};
use crate::server::core::providers::{catalog, images as image_models};
use crate::server::core::upstream::cancellation;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::{ForwardOutcome, ForwardRequest};
use crate::server::errors::GatewayError;
use crate::server::{logging, ServerState};

use super::disconnect_guard::DisconnectGuard;
use super::pipeline::{self, RecordContext};

pub use super::image_assets::download;

pub(super) fn parameter_supported(field: &str) -> bool {
    matches!(
        field,
        "model"
            | "prompt"
            | "n"
            | "size"
            | "response_format"
            | "output_format"
            | "output_compression"
            | "quality"
            | "style"
            | "background"
            | "moderation"
            | "user"
            | "stream"
            | "partial_images"
    )
}

fn validate(
    payload: &mut Value,
    operation: image_models::ImageOperation,
) -> Result<(), GatewayError> {
    if !payload.is_object() {
        return Err(GatewayError::bad_request("请求体必须是 JSON 对象"));
    }
    let prompt = payload
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.trim().is_empty())
        .ok_or_else(|| GatewayError::bad_request("prompt 必须是非空字符串"))?;
    if prompt.chars().count() > 32_000 {
        return Err(GatewayError::bad_request("prompt 最多 32000 个字符"));
    }
    if payload
        .get("model")
        .is_some_and(|model| !model.is_string() && !model.is_null())
    {
        return Err(GatewayError::bad_request("model 必须是字符串"));
    }
    let n = match payload.get("n").filter(|n| !n.is_null()) {
        Some(n) => n
            .as_u64()
            .filter(|n| (1..=10).contains(n))
            .ok_or_else(|| GatewayError::bad_request("n 必须是 1 到 10 之间的整数"))?,
        None => 1,
    };
    payload["n"] = json!(n);
    for (field, values) in [
        ("response_format", &["b64_json", "url"][..]),
        ("output_format", &["png", "jpeg", "webp"][..]),
        ("background", &["auto", "opaque", "transparent"][..]),
        (
            "quality",
            &[
                "auto", "standard", "hd", "low", "medium", "high", "xhigh", "max",
            ][..],
        ),
        ("style", &["natural", "vivid"][..]),
        ("moderation", &["auto", "low"][..]),
    ] {
        if let Some(value) = payload.get(field).filter(|value| !value.is_null()) {
            if !value.as_str().is_some_and(|value| values.contains(&value)) {
                return Err(GatewayError::bad_request(format!(
                    "{field} 必须是 {}",
                    values.join(" 或 ")
                )));
            }
        }
    }
    if let Some(size) = payload.get("size").filter(|size| !size.is_null()) {
        let valid = size.as_str().is_some_and(|size| {
            size == "auto"
                || size
                    .split_once('x')
                    .and_then(|(width, height)| {
                        Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?))
                    })
                    .is_some_and(|(width, height)| {
                        (1..=8192).contains(&width) && (1..=8192).contains(&height)
                    })
        });
        if !valid {
            return Err(GatewayError::bad_request(
                "size 必须是 auto 或 WIDTHxHEIGHT，边长不得超过 8192",
            ));
        }
    }
    if let Some(compression) = payload
        .get("output_compression")
        .filter(|value| !value.is_null())
    {
        if !compression.as_u64().is_some_and(|value| value <= 100) {
            return Err(GatewayError::bad_request(
                "output_compression 必须是 0 到 100 的整数",
            ));
        }
        if payload.get("output_format").and_then(Value::as_str) != Some("jpeg") {
            return Err(GatewayError::bad_request(
                "output_compression 目前仅支持 output_format=jpeg",
            ));
        }
    }
    if payload
        .get("stream")
        .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
    {
        return Err(GatewayError::bad_request(
            "当前图片生成仅支持非流式响应，请使用 stream=false",
        ));
    }
    if payload
        .get("partial_images")
        .is_some_and(|value| !value.is_null() && value.as_u64() != Some(0))
    {
        return Err(GatewayError::bad_request(
            "当前图片生成不支持 partial_images",
        ));
    }
    let editing = operation == image_models::ImageOperation::Edit;
    if ["mask", "input_fidelity"]
        .iter()
        .any(|key| payload.get(*key).is_some_and(|value| !value.is_null()))
    {
        return Err(GatewayError::bad_request(
            "当前参考图编辑通道不支持 mask 或 input_fidelity，请在 prompt 中描述修改范围",
        ));
    }
    if payload.get("image").is_some() || (!editing && payload.get("images").is_some()) {
        return Err(GatewayError::bad_request(
            "参考图请通过 /v1/images/edits 上传 image 文件或 JSON images[].image_url",
        ));
    }
    if editing
        && !payload
            .get("images")
            .and_then(Value::as_array)
            .is_some_and(|images| !images.is_empty() && images.len() <= 16)
    {
        return Err(GatewayError::bad_request("图片编辑需要 1 到 16 张参考图"));
    }
    if payload.get("response_format").is_none_or(Value::is_null) {
        payload["response_format"] = json!("b64_json");
    }
    if let Some(field) = payload.as_object().and_then(|object| {
        object.keys().find(|key| {
            !parameter_supported(key)
                && !(editing && matches!(key.as_str(), "images" | "mask" | "input_fidelity"))
        })
    }) {
        return Err(GatewayError::bad_request(format!(
            "当前图片生成不支持参数 {field}"
        )));
    }
    if payload
        .get("user")
        .is_some_and(|value| !value.is_string() && !value.is_null())
    {
        return Err(GatewayError::bad_request("user 必须是字符串"));
    }
    Ok(())
}

fn resolve_model(
    state: &ServerState,
    payload: &mut Value,
    scope: Option<&KeyScope>,
    operation: image_models::ImageOperation,
) -> Result<String, GatewayError> {
    if pipeline::model_field_text(payload).trim().is_empty() {
        let models = catalog::models_response(state.store(), scope);
        let default = models
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    if !image_models::model_is_image(item) {
                        return false;
                    }
                    let model = crate::server::core::models::model_id(item);
                    image_models::operation_providers(&model, operation)
                        .iter()
                        .any(|provider| key_scope::allows_provider(scope, provider))
                })
            })
            .and_then(|item| item.get("id"))
            .cloned();
        let Some(default) = default else {
            return Err(GatewayError::with_status(
                503,
                format!(
                    "没有可用的{}模型：请添加并启用支持该能力的账号，并更新提供商插件和模型目录",
                    operation.label()
                ),
            ));
        };
        payload["model"] = default;
    }
    if let Some(model) = payload.get_mut("model") {
        if let Some(name) = model.as_str() {
            *model = json!(name.trim());
        }
    }
    let requested = pipeline::resolve_image_model(state, payload, scope)?;
    if !key_scope::allows_model(scope, &requested) {
        return Err(GatewayError::with_status(
            404,
            format!("模型 '{requested}' 不在这把网关 Key 的可用模型列表里"),
        )
        .with_code("model_not_found"));
    }
    let candidates = image_models::operation_providers(&requested, operation);
    if !candidates
        .iter()
        .any(|provider| key_scope::allows_provider(scope, provider))
    {
        return Err(GatewayError::bad_request(format!(
            "模型 '{requested}' 没有获准使用的{}通道，请从 GET /v1/models 选择对应能力的模型并更新插件", operation.label()
        )).with_code(if operation == image_models::ImageOperation::Edit {
            "image_editing_not_supported"
        } else { "image_generation_not_supported" }));
    }
    Ok(requested)
}

pub async fn generations(
    State(state): State<ServerState>,
    headers: HeaderMap,
    scope: Option<Extension<KeyScope>>,
    key_identity: Option<Extension<crate::server::request_stats::ApiKeyIdentity>>,
    body: Bytes,
) -> Response {
    let started_at = logging::now_ms();
    let source = pipeline::request_source(&headers, key_identity);
    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(payload) => payload,
        Err(_) => {
            let error = GatewayError::bad_request("请求体必须是有效 JSON 对象");
            pipeline::record_early_failure(&state, started_at, "", "", &source, &error);
            return error.payload_response();
        }
    };
    respond(
        execute(
            &state,
            payload,
            ImageExecution {
                operation: image_models::ImageOperation::Generate,
                headers,
                scope: scope.map(|Extension(scope)| scope),
                source,
                started_at,
                is_assistant: false,
            },
        )
        .await,
    )
}

pub async fn edits(
    State(state): State<ServerState>,
    headers: HeaderMap,
    scope: Option<Extension<KeyScope>>,
    key_identity: Option<Extension<crate::server::request_stats::ApiKeyIdentity>>,
    request: axum::extract::Request,
) -> Response {
    let started_at = logging::now_ms();
    let source = pipeline::request_source(&headers, key_identity);
    let payload = match super::image_input::parse(request, &state).await {
        Ok(payload) => payload,
        Err(error) => {
            pipeline::record_early_failure(&state, started_at, "", "", &source, &error);
            return error.payload_response();
        }
    };
    respond(
        execute(
            &state,
            payload,
            ImageExecution {
                operation: image_models::ImageOperation::Edit,
                headers,
                scope: scope.map(|Extension(scope)| scope),
                source,
                started_at,
                is_assistant: false,
            },
        )
        .await,
    )
}

fn respond(result: Result<Value, GatewayError>) -> Response {
    match result {
        Ok(body) => pipeline::json_response(body),
        Err(error) => error.payload_response(),
    }
}

pub(super) struct ImageExecution {
    pub operation: image_models::ImageOperation,
    pub headers: HeaderMap,
    pub scope: Option<KeyScope>,
    pub source: crate::server::request_stats::RequestSource,
    pub started_at: i64,
    pub is_assistant: bool,
}

pub(super) async fn execute(
    state: &ServerState,
    mut payload: Value,
    input: ImageExecution,
) -> Result<Value, GatewayError> {
    let ImageExecution {
        operation,
        headers,
        scope,
        source,
        started_at,
        is_assistant,
    } = input;
    let client_model = pipeline::model_field_text(&payload);
    let state_copy = state.clone();
    let scope_copy = scope.clone();
    let (prepared, resolved) = crate::server::core::blocking::run(move || {
        let resolved = validate(&mut payload, operation)
            .and_then(|()| resolve_model(&state_copy, &mut payload, scope_copy.as_ref(), operation));
        (payload, resolved)
    }).await?;
    payload = prepared;
    let model = match resolved {
        Ok(model) => model,
        Err(error) => {
            pipeline::record_early_failure(state, started_at, &client_model, "", &source, &error);
            return Err(error);
        }
    };
    if operation == image_models::ImageOperation::Edit {
        if let Err(error) = super::image_input::prepare(&mut payload).await {
            pipeline::record_early_failure(state, started_at, &client_model, "", &source, &error);
            return Err(error);
        }
    }
    let raw = serde_json::to_vec(&payload)
        .map_err(|_| GatewayError::bad_request("图片请求无法序列化"))?;
    pipeline::write_debug_files(
        &raw,
        "POST",
        operation.path(),
        &pipeline::user_agent_of(&headers),
    );
    let telemetry = Arc::new(RequestTelemetry::with_id());
    let id = telemetry.id();
    super::request_prepare::record_started(
        state.request_stats(),
        &id,
        started_at,
        &model,
        &client_model,
        "",
        &source,
        is_assistant,
    ).await;
    if let Some(token) = cancellation::register(&id) {
        telemetry.set_cancel_token(token);
    }
    let mut guard = DisconnectGuard::new(state.request_stats(), id.clone());
    telemetry.set_live_sink(pipeline::live_row_sink(
        state.request_stats(),
        id,
        started_at,
    ));
    let raw_request = pipeline::capture_raw_body(&raw);
    let request = ForwardRequest {
        body: payload.clone(),
        stream: false,
        dedupe_key: String::new(),
        client_headers: if is_assistant {
            HeaderMap::new()
        } else {
            headers.clone()
        },
        telemetry: telemetry.clone(),
        allowed_providers: scope,
        pinned_account: None,
        ignore_model_gate: false,
        proxy_override: None,
    };
    let result = match operation {
        image_models::ImageOperation::Generate => state.upstream().generate_images(request).await,
        image_models::ImageOperation::Edit => state.upstream().edit_images(request).await,
    };
    let outcome = match result {
        Ok(ForwardOutcome::Completion { body }) => {
            super::image_output::normalize(state, &payload, body, &headers, &telemetry).await
        }
        Ok(ForwardOutcome::Stream { .. }) => Err(GatewayError::with_status(
            502,
            "插件错误地把图片响应当作对话流",
        )),
        Err(error) => Err(error),
    };
    let (status, raw_response, error) = match &outcome {
        Ok(body) => (200, pipeline::capture_raw_json(body), None),
        Err(error) => (
            i64::from(error.http_status().as_u16()),
            None,
            Some(error.message.clone()),
        ),
    };
    pipeline::record_entry(
        &RecordContext {
            stats: state.request_stats(),
            telemetry,
            started_at,
            model,
            client_model,
            client_reasoning: String::new(),
            status,
            raw_request,
            raw_response,
            is_test: is_assistant,
            source,
        },
        error,
    );
    guard.complete();
    outcome
}
