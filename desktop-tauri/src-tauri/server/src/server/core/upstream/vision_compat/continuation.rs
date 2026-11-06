use std::sync::Arc;
use serde_json::Value;

use crate::server::errors::GatewayError;
use super::super::{payload::ProviderContext, route_context::RequestRouting, ForwardRequest};
use super::targets::Destination;

pub(crate) struct VisionContinuation {
    body: Value,
    original_messages: usize,
    destination: Destination,
    requested: String,
}

pub(super) fn remember(ctx: &ProviderContext<'_>, body: &Value, destination: &Destination) {
    if !crate::server::config::auto_continue() { return; }
    let original_messages = ctx.vision.as_ref().map(|vision| vision.original_messages)
        .unwrap_or_else(|| ctx.body.get("messages").and_then(Value::as_array).map_or(0, Vec::len));
    ctx.telemetry.set_vision_continuation(Arc::new(VisionContinuation {
        body: body.clone(), original_messages, destination: destination.clone(),
        requested: ctx.body.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
    }));
}

pub(in crate::server::core::upstream) struct ResumedVision {
    pub routing: RequestRouting,
    pub provider: String,
    pub model: String,
    pub requested: String,
}

pub(in crate::server::core::upstream) fn resume(request: &mut ForwardRequest) -> Result<Option<ResumedVision>, GatewayError> {
    let Some(saved) = request.telemetry.vision_continuation() else { return Ok(None); };
    let extra = request.body.get("messages").and_then(Value::as_array)
        .and_then(|messages| messages.get(saved.original_messages..))
        .filter(|messages| !messages.is_empty()).ok_or_else(|| GatewayError::new("图片兼容续写缺少新增消息，已停止"))?;
    let mut body = saved.body.clone();
    let messages = body.get_mut("messages").and_then(Value::as_array_mut)
        .ok_or_else(|| GatewayError::new("图片兼容续写缺少原始对话，已停止"))?;
    messages.extend_from_slice(extra);
    request.body = body;
    Ok(Some(ResumedVision { routing: saved.destination.routing.clone(), provider: saved.destination.target.provider.clone(),
        model: saved.destination.target.model.clone(), requested: saved.requested.clone() }))
}
