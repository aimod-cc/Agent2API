//! 同协议发送只在原生体上修改路由参数，不复用有损的 Chat 历史。

use serde_json::Value;

use crate::server::core::prompt::PromptMode;
use crate::server::core::protocol::native::{NativeRequest, WireProtocol};
use crate::server::errors::GatewayError;

use super::payload::ProviderContext;

pub(super) fn filter_candidates(
    ctx: &ProviderContext<'_>,
    mut candidates: Vec<String>,
) -> Result<Vec<String>, GatewayError> {
    let Some(native) = ctx.native.filter(|native| native.requires_native()) else {
        return Ok(candidates);
    };
    candidates.retain(|provider| {
        crate::server::core::custom_providers::get(provider).and_then(|owner| {
            owner
                .get("protocol")
                .and_then(Value::as_str)
                .and_then(WireProtocol::from_provider)
        }) == Some(native.protocol)
            || crate::server::core::plugin_host::script::native::protocol(provider,
                ctx.direct_model.as_deref().unwrap_or_else(|| ctx.body["model"].as_str().unwrap_or(""))) == Some(native.protocol)
            || crate::server::core::plugin_host::script::native::has_protocol(provider, native.protocol)
    });
    if candidates.is_empty() {
        return Err(GatewayError::bad_request(format!(
            "当前请求需要原生 {} 上游，但当前权限和路由范围内没有对应出口：{}",
            native.protocol.as_str(),
            native.translation_error.as_deref().unwrap_or("无法转换"),
        ))
        .with_code("native_protocol_required"));
    }
    Ok(candidates)
}

pub(super) fn eligible(ctx: &ProviderContext<'_>, provider: &str) -> Result<bool, GatewayError> {
    let Some(native) = ctx.native else {
        return Ok(false);
    };
    let protocol = crate::server::core::custom_providers::get(provider).and_then(|owner| {
        owner
            .get("protocol")
            .and_then(Value::as_str)
            .and_then(WireProtocol::from_provider)
    });
    let same = protocol == Some(native.protocol);
    if native.requires_native() && !same {
        return Err(GatewayError::bad_request(format!(
            "当前请求包含只能由原生 {} 接口处理的字段，不能转发到其他协议：{}",
            native.protocol.as_str(),
            native.translation_error.as_deref().unwrap_or("无法转换"),
        ))
        .with_code("native_protocol_required"));
    }
    Ok(same)
}

pub(super) fn prepare(
    ctx: &ProviderContext<'_>,
    native: &NativeRequest,
    provider: &str,
    account: Option<&Value>,
    degraded: bool,
) -> super::payload::SendBody<'static> {
    let mut body = native.routed(ctx.body);
    if ctx.compact_routed || ctx.title_routed {
        crate::server::core::compact_routing::parameters::clear_source_reasoning(&mut body);
    }
    let preserve_content = native.protocol == WireProtocol::Anthropic;
    if preserve_content || native.requires_native() {
        // 第一轮的内容同样会被后续签名绑定，不能等出现签名才停止改写。
        if !ctx.preserve_system_prompt
            && ctx.prompt.for_provider(provider).mode != PromptMode::Passthrough
            || ctx.sanitize.applies_to(provider)
            || ctx.image_compress.enabled
            || degraded
        {
            ctx.telemetry.note_attempt_notice(if preserve_content {
                "原生 Anthropic 转发保留提示词、消息与图片，跳过内容改写以保持思考签名连续性"
            } else {
                "原生服务端状态请求保留内容，不对已有会话进行内容改写"
            });
        }
    } else {
        let prompt = ctx.prompt.for_provider(provider);
        if !ctx.preserve_system_prompt {
            apply_prompt(
                &mut body,
                native.protocol,
                prompt.mode,
                prompt.text_for(degraded),
                degraded,
            );
        }
        if ctx.sanitize.applies_to(provider) {
            if let Some((next, hits)) = crate::server::core::sanitize::sanitize_native_body(
                &body,
                native.protocol,
                &ctx.sanitize.rules,
            ) {
                body = next;
                ctx.telemetry.note_sensitive_hits(&hits);
            }
        }
        if ctx.image_compress.enabled {
            let stats = crate::server::core::blocking::in_place(|| {
                super::image_compress::compress_native_images(
                    &mut body,
                    native.protocol,
                    &ctx.image_compress,
                )
            });
            if stats.changed() {
                crate::server::logging::verbose(
                    "[Upstream]",
                    &format!(
                        "原生会话图片压缩：{} 张 provider={provider} {}KB -> {}KB",
                        stats.compressed,
                        stats.bytes_before / 1024,
                        stats.bytes_after / 1024,
                    ),
                );
            }
        }
    }
    let requested = ctx.body.get("model").and_then(Value::as_str).unwrap_or("");
    let wire_model = ctx
        .direct_model
        .clone()
        .or_else(|| {
            account
                .and_then(crate::server::core::routing_groups::candidate_model)
                .map(str::to_string)
        })
        .unwrap_or_else(|| requested.to_string());
    if !wire_model.is_empty() {
        body["model"] = wire_model.clone().into();
    }
    if ctx.compact_routed {
        let actual_model = if ctx.routing.group.is_some() || ctx.direct_model.is_some() {
            wire_model.clone()
        } else {
            crate::server::core::providers::custom::forward::cooldown_model(provider, &wire_model)
        };
        let limit = crate::server::core::routing_groups::catalog_model(provider, &actual_model)
            .and_then(|item| item.get("maxOutputTokens").and_then(Value::as_u64))
            .filter(|limit| *limit > 0);
        if let Some(limit) = limit {
            for key in ["max_tokens", "max_output_tokens"] {
                if body
                    .get(key)
                    .and_then(Value::as_u64)
                    .is_some_and(|value| value > limit)
                {
                    body[key] = limit.into();
                }
            }
        }
    }
    super::payload::SendBody {
        body: std::borrow::Cow::Owned(body),
        wire_model,
        wire_reasoning: None,
    }
}

fn apply_prompt(
    body: &mut Value,
    protocol: WireProtocol,
    mode: PromptMode,
    text: &str,
    degraded: bool,
) {
    if text.is_empty() || mode == PromptMode::Passthrough && !degraded {
        return;
    }
    let append = mode == PromptMode::Append && !degraded;
    let key = match protocol {
        WireProtocol::Anthropic => "system",
        WireProtocol::Responses => "instructions",
        WireProtocol::Chat => return,
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if append {
        match object.get_mut(key) {
            Some(Value::String(current)) if !current.is_empty() => {
                current.push_str("\n\n");
                current.push_str(text);
            }
            Some(Value::Array(parts)) if protocol == WireProtocol::Anthropic => {
                parts.push(serde_json::json!({ "type": "text", "text": text }));
            }
            _ => {
                object.insert(key.to_string(), text.into());
            }
        }
    } else {
        object.insert(key.to_string(), text.into());
        if protocol == WireProtocol::Responses {
            if let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) {
                input.retain(|item| {
                    !matches!(
                        item.get("role").and_then(Value::as_str),
                        Some("system" | "developer")
                    )
                });
            }
        }
    }
}
