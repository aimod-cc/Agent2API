//! 隐私脱敏模块：在请求离开本机前识别并遮罩密钥、钱包私钥/助记词、个人隐私信息与自定义保密词，
//! 并在响应流 / JSON 中透明还原（原值仅驻留内存，绝不落盘或写入日志）。

mod detectors;
mod rules;
mod sse_frame;
mod stream;
mod stream_native;
mod vault;
mod walker;

#[cfg(test)]
mod tests;

use serde_json::{json, Value};

pub use detectors::{mask_text, MaskTextResult, PrivacyHitRecord};
pub use rules::{
    category_catalog, default_categories, kind_label_zh, normalize_kind, CompiledPrivacySettings,
    CustomRuleMatchType, PrivacyAction, PrivacyCustomRule, PrivacyProviderMode, PrivacySettings,
    MAX_ALLOWLIST, MAX_CUSTOM_RULES, MAX_KIND_CHARS, MAX_NAME_CHARS, MAX_PREFIX_CHARS,
    MAX_PROVIDERS, MAX_REGEX_CHARS, MAX_TEST_TEXT_BYTES, MAX_WORDS, MAX_WORD_CHARS,
    MIN_PREFIX_CHARS, PERSONAL_CATEGORIES,
};
pub use stream::{unmask_outcome, unmask_outcome_with_options, StreamChannelRestorer};
pub use vault::{PrivacyVault, RequestVault};
pub use walker::{
    is_stateful_responses_request, mask_chat_body, mask_native_body, request_expects_json_output,
    unmask_json_response, unmask_json_response_with_mode, PrivacyBodyOutcome,
};

/// 试一试实时预览：使用独立临时保险库，不污染全局脱敏映射。
/// 支持直接输入普通文本，也支持粘贴整段 JSON 预览按敏感字段名遮罩的效果。
pub fn preview_text(text: &str, compiled: &CompiledPrivacySettings) -> Value {
    let vault = if compiled.settings.ephemeral_placeholders {
        PrivacyVault::ephemeral_opaque()
    } else {
        PrivacyVault::ephemeral()
    };

    // 若用户输入的是完整 JSON 对象，优先模拟工具 JSON 按字段名遮罩预览，更贴合 Agent 场景。
    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        if let Ok(Value::Object(_)) = serde_json::from_str::<Value>(trimmed) {
            let wrapped = json!({
                "messages": [{
                    "role": "tool",
                    "content": trimmed,
                }]
            });
            match mask_chat_body(&wrapped, compiled, &vault) {
                PrivacyBodyOutcome::Unchanged => {
                    return json!({
                        "maskedText": text,
                        "restoredText": text,
                        "blocked": false,
                        "blockMessage": Value::Null,
                        "matches": [],
                    });
                }
                PrivacyBodyOutcome::Warned { hits } => {
                    return preview_payload(text.to_string(), text.to_string(), None, &hits);
                }
                PrivacyBodyOutcome::Masked { body, hits } => {
                    let masked_raw = body
                        .pointer("/messages/0/content")
                        .and_then(Value::as_str)
                        .unwrap_or(text)
                        .to_string();
                    let restored_text = if compiled.settings.reversible {
                        let mut dummy = json!({
                            "choices": [{ "message": { "content": masked_raw.clone() } }]
                        });
                        let _ = unmask_json_response_with_mode(
                            &mut dummy,
                            crate::server::core::protocol::native::WireProtocol::Chat,
                            &vault,
                            true,
                        );
                        dummy
                            .pointer("/choices/0/message/content")
                            .and_then(Value::as_str)
                            .unwrap_or(&masked_raw)
                            .to_string()
                    } else {
                        masked_raw.clone()
                    };
                    return preview_payload(masked_raw, restored_text, None, &hits);
                }
                PrivacyBodyOutcome::Blocked { message, hits } => {
                    return preview_payload(
                        text.to_string(),
                        text.to_string(),
                        Some(message),
                        &hits,
                    );
                }
            }
        }
    }

    match mask_text(text, compiled, &vault) {
        MaskTextResult::Unchanged => json!({
            "maskedText": text,
            "restoredText": text,
            "blocked": false,
            "blockMessage": Value::Null,
            "matches": [],
        }),
        MaskTextResult::Handled {
            text: replacement,
            hits,
            blocked_message,
        } => {
            let masked_text = replacement.unwrap_or_else(|| text.to_string());
            let restored_text = if blocked_message.is_none() && compiled.settings.reversible {
                vault.restore_text(&masked_text, false).0
            } else {
                masked_text.clone()
            };
            preview_payload(masked_text, restored_text, blocked_message, &hits)
        }
    }
}

/// 供调试落盘使用：对请求体做一次性临时脱敏，避免把敏感原文直接写入 `debug/last-request.json`。
pub fn redact_debug_body(bytes: &[u8], path: &str, compiled: &CompiledPrivacySettings) -> Vec<u8> {
    if !compiled.settings.enabled {
        return bytes.to_vec();
    }
    let Ok(parsed) = serde_json::from_slice::<Value>(bytes) else {
        return b"{\"_redacted\":\"privacy redaction enabled; non-JSON body omitted\"}".to_vec();
    };
    // 调试落盘强制使用遮罩（即便策略是 warn 或 block），确保磁盘上不留敏感明文。
    let mut mask_only = compiled.settings.clone();
    mask_only.action = PrivacyAction::Mask;
    mask_only.reversible = false;
    mask_only.kind_actions.clear();
    for rule in &mut mask_only.custom_rules {
        rule.action = Some(PrivacyAction::Mask);
    }
    let Ok(safe_compiled) = CompiledPrivacySettings::compile(mask_only) else {
        return b"{\"_redacted\":\"privacy redaction enabled\"}".to_vec();
    };
    let vault = PrivacyVault::ephemeral();
    let protocol = match path {
        "/v1/responses" => crate::server::core::protocol::native::WireProtocol::Responses,
        "/v1/messages" => crate::server::core::protocol::native::WireProtocol::Anthropic,
        "/v1/chat/completions" => crate::server::core::protocol::native::WireProtocol::Chat,
        _ if parsed.get("input").is_some() || parsed.get("instructions").is_some() => {
            crate::server::core::protocol::native::WireProtocol::Responses
        }
        _ if parsed.get("system").is_some() => {
            crate::server::core::protocol::native::WireProtocol::Anthropic
        }
        _ => crate::server::core::protocol::native::WireProtocol::Chat,
    };
    match mask_native_body(&parsed, protocol, &safe_compiled, &vault) {
        PrivacyBodyOutcome::Masked { body, .. } => {
            serde_json::to_vec(&body).unwrap_or_else(|_| b"{\"_redacted\":true}".to_vec())
        }
        PrivacyBodyOutcome::Unchanged | PrivacyBodyOutcome::Warned { .. } => bytes.to_vec(),
        PrivacyBodyOutcome::Blocked { .. } => b"{\"_redacted\":true}".to_vec(),
    }
}

fn preview_payload(
    masked_text: String,
    restored_text: String,
    blocked_message: Option<String>,
    hits: &[PrivacyHitRecord],
) -> Value {
    let matches: Vec<Value> = hits
        .iter()
        .map(|hit| {
            json!({
                "kind": hit.kind,
                "label": kind_label_zh(&hit.kind),
                "count": hit.count,
                "action": hit.action.as_str(),
            })
        })
        .collect();
    json!({
        "maskedText": masked_text,
        "restoredText": restored_text,
        "blocked": blocked_message.is_some(),
        "blockMessage": blocked_message,
        "matches": matches,
    })
}
