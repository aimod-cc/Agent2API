//! Magpie siwcBody + 实际调用的 sessionHeaders / conversationKey。
//! 仅构造 SIWC 的 Responses 请求，不借用 Codex 后端、UA 或专用系统提示词。

use axum::http::{HeaderMap, HeaderValue};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::core::protocol::{
    responses_outbound::responses_request_from_chat, strip_internal_fields,
};
use crate::server::core::providers::adapter::{ChatRequestPlan, UpstreamResponse};
use crate::server::errors::GatewayError;

use super::{account, API_BASE, USER_AGENT};

const DROPPED_FIELDS: &[&str] = &[
    "background",
    "conversation",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "moderation",
    "multi_agent",
    "prompt",
    "prompt_cache_retention",
    "safety_identifier",
    "temperature",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
    "previous_response_id",
];
const HOSTED_TOOLS: &[&str] = &[
    "image_generation",
    "file_search",
    "code_interpreter",
    "computer_use",
    "computer_use_preview",
    "computer",
    "mcp",
    "tool_search",
];

pub(super) fn chat(
    account: &Value,
    body: &Value,
    headers: &HeaderMap,
) -> Result<ChatRequestPlan, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| GatewayError::bad_request("ChatGPT 请求缺少模型"))?;
    let body = responses_request_from_chat(body, model).map_err(GatewayError::bad_request)?;
    build_native_request(account, &body, headers)
}

/// 供公共原生分派调用；直接保留 Responses 的输入项、工具、include 与加密思考。
pub fn build_native_request(
    account: &Value,
    body: &Value,
    headers: &HeaderMap,
) -> Result<ChatRequestPlan, GatewayError> {
    // 标准账号库把凭证放在 secret，不依赖旧 session.auth 的顶层镜像。
    // 指定 id 时必须重读本家凭证，拒绝跨家借用及已失效/尚未验签的令牌。
    let saved = match account
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Some(account::load(&account::shared()?, id)?.1),
        None => None,
    };
    if saved.as_ref().is_some_and(|credentials| {
        credentials.needs_relogin
            || credentials.identity_pending
            || credentials.expires_at <= crate::server::logging::now_ms()
    }) {
        return Err(GatewayError::with_status(
            401,
            "ChatGPT 凭证不可用，请刷新凭证或重新授权",
        ));
    }
    let token = saved
        .as_ref()
        .map(|credentials| credentials.access_token.as_str())
        .or_else(|| {
            account
                .pointer("/auth/accessToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
        })
        .or_else(|| {
            account
                .pointer("/secret/accessToken")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            account
                .pointer("/pluginSecret/accessToken")
                .and_then(Value::as_str)
        })
        .or_else(|| account.get("accessToken").and_then(Value::as_str))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| GatewayError::with_status(401, "ChatGPT 缺少可用登录凭证，请重新授权"))?;
    let authorization = format!("Bearer {token}");
    HeaderValue::from_str(&authorization)
        .map_err(|_| GatewayError::bad_request("ChatGPT 登录凭证格式无效"))?;
    let mut request = body.clone();
    normalize(&mut request)?;
    if let Some(credentials) = &saved {
        super::models::check_model(&credentials.identity(), request["model"].as_str().unwrap_or(""))?;
    }
    let mut outgoing = vec![
        ("Authorization".to_string(), authorization),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "text/event-stream".to_string()),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
    ];
    // 不转发客户端 Authorization、Cookie、Codex originator 或其他身份头。
    let cache_key = request
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty());
    for name in ["session-id", "thread-id"] {
        let value = cache_key.or_else(|| headers.get(name).and_then(|value| value.to_str().ok()));
        if let Some(value) = value.filter(|value| value.len() <= 1024) {
            if HeaderValue::from_str(value).is_ok() {
                outgoing.push((name.to_string(), value.to_string()));
            }
        }
    }
    // 先让 Chat→Responses 转换恢复工具和加密思考，再清理最终发送体中的内部字段。
    let request = strip_internal_fields(&request).into_owned();
    let mut plan = ChatRequestPlan::chat(format!("{API_BASE}/responses"), outgoing, request);
    plan.response = UpstreamResponse::Responses;
    Ok(plan)
}

fn normalize(body: &mut Value) -> Result<(), GatewayError> {
    let fields = body
        .as_object_mut()
        .ok_or_else(|| GatewayError::bad_request("ChatGPT 请求必须是 JSON 对象"))?;
    if fields
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(|model| model.trim().is_empty())
    {
        return Err(GatewayError::bad_request("ChatGPT 请求缺少模型"));
    }
    // SIWC 固定 store=false；先拒绝历史引用，不能删掉它们后把缺失历史的请求发出去。
    for key in ["previous_response_id", "conversation", "prompt"] {
        if fields
            .get(key)
            .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        {
            return Err(GatewayError::bad_request(format!(
                "ChatGPT 订阅不保存服务端历史，不能使用 {key}，请在 input 中发送完整历史和提示词"
            ))
            .with_code("chatgpt_stateless_history_required"));
        }
    }
    if fields.get("background").and_then(Value::as_bool) == Some(true) {
        return Err(GatewayError::bad_request(
            "ChatGPT 订阅不支持后台生成，请使用普通或流式请求",
        ));
    }
    for key in DROPPED_FIELDS {
        fields.remove(*key);
    }
    fields.insert("store".to_string(), Value::Bool(false));
    fields.insert("stream".to_string(), Value::Bool(true));
    if let Some(Value::String(text)) = fields.get("input") {
        let input = json!([{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": text }] }]);
        fields.insert("input".to_string(), input);
    }
    if !fields.get("input").is_some_and(Value::is_array) {
        return Err(GatewayError::bad_request("ChatGPT 请求缺少 input 数组"));
    }
    if let Some(input) = fields.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            if item.get("role").and_then(Value::as_str) == Some("system") {
                item.insert("role".to_string(), json!("developer"));
            }
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                if let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut) {
                    own_tools(tools);
                }
            }
        }
    }
    let remove_tools = fields
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .is_some_and(|tools| {
            own_tools(tools);
            tools.is_empty()
        });
    if remove_tools {
        fields.remove("tools");
        fields.remove("tool_choice");
    }
    if fields
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .is_none_or(|key| key.is_empty())
    {
        let instructions = fields
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(input) = fields.get("input").and_then(Value::as_array) {
            if let Some(key) = conversation_key(instructions, input) {
                fields.insert("prompt_cache_key".to_string(), Value::String(key));
            }
        }
    }
    Ok(())
}

fn own_tools(tools: &mut Vec<Value>) {
    tools.retain(|tool| {
        !tool
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| HOSTED_TOOLS.contains(&kind))
    });
}

fn conversation_key(instructions: &str, input: &[Value]) -> Option<String> {
    let mut hash = Sha256::new();
    hash.update(instructions.as_bytes());
    for item in input {
        hash.update(serde_json::to_vec(item).ok()?);
        if item.get("role").and_then(Value::as_str) == Some("user") {
            let sum = format!("{:x}", hash.finalize());
            return Some(format!(
                "{}-{}-{}-{}-{}",
                &sum[..8],
                &sum[8..12],
                &sum[12..16],
                &sum[16..20],
                &sum[20..32]
            ));
        }
    }
    None
}

pub(super) fn public_view(secret: &Value) -> Result<Value, GatewayError> {
    account::Credentials::parse(secret).map(|credentials| credentials.public())
}
