//! Codex 专属 Responses 请求整理，取自 Magpie codex_request.go 与 gateway/codex_backend.go。
//! Chat 转换复用公共层；原生请求不经 Chat，工具、密文思考与原生压缩项原样保留。
//! 未提供 instructions 时使用该模型目录给出的基础指令；不额外冒充客户端或覆盖用户指令。

use axum::http::{HeaderMap, HeaderValue};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::protocol::responses_outbound::responses_request_from_chat;
use crate::server::core::providers::adapter::{ChatRequestPlan, ReasoningPatch, UpstreamResponse};
use crate::server::errors::GatewayError;

use super::{
    account::{self, Tokens},
    http, models,
};

pub(super) fn chat(
    account: &Value,
    body: &Value,
    headers: &HeaderMap,
) -> Result<ChatRequestPlan, GatewayError> {
    let model = account::text(body, &["model"]);
    let mut converted =
        responses_request_from_chat(body, model).map_err(GatewayError::bad_request)?;
    // 公共 Chat 转换器不认识的会话缓存键，在本家的 Responses 发送前保留下来。
    for key in ["prompt_cache_key", "include"] {
        if let Some(value) = body.get(key) {
            converted[key] = value.clone();
        }
    }
    if converted.get("reasoning").is_none() {
        if let Some(reasoning) = body.get("reasoning").filter(|value| value.is_object()) {
            converted["reasoning"] = reasoning.clone();
        }
    }
    build(account, &converted, headers, false)
}

pub fn build_native_request(
    account: &Value,
    body: &Value,
    client_headers: &HeaderMap,
) -> Result<ChatRequestPlan, GatewayError> {
    build(account, body, client_headers, true)
}

fn build(
    account: &Value,
    body: &Value,
    client_headers: &HeaderMap,
    native: bool,
) -> Result<ChatRequestPlan, GatewayError> {
    let secret = account
        .get("secret")
        .or_else(|| account.get("pluginSecret"))
        .unwrap_or(account);
    if secret.get("needsRelogin").and_then(Value::as_bool) == Some(true) {
        return Err(
            GatewayError::with_status(401, "Codex 登录授权已失效，请重新授权")
                .with_code("codex_relogin_required"),
        );
    }
    let tokens = Tokens::read(account)?;
    let model = account::text(body, &["model"]);
    if model.is_empty() {
        return Err(GatewayError::bad_request("Codex 请求缺少模型"));
    }
    let identity = tokens.identity();
    let catalog = models::entry(&identity, model);
    if !models::known(&identity) {
        return Err(GatewayError::bad_request(
            "尚未取得这个 Codex 账号的模型权限，请先获取模型目录",
        )
        .with_code("account_model_permissions_unknown"));
    }
    if catalog.is_none() {
        return Err(GatewayError::bad_request(
            "这个 Codex 账号的目录未包含所选模型，请换用有权限的账号或刷新目录",
        )
        .with_code("account_model_unavailable"));
    }
    http::observe_client(client_headers);
    let mut body = body
        .as_object()
        .cloned()
        .ok_or_else(|| GatewayError::bad_request("Codex 请求必须是 JSON 对象"))?;
    // store=false 不可能从 response id 找回历史；不可像参考的宽松转换那样静默丢历史。
    for key in ["previous_response_id", "conversation", "prompt"] {
        if body
            .get(key)
            .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        {
            return Err(GatewayError::bad_request(format!(
                "Codex 订阅不保存服务端历史，不能使用 {key}，请在 input 中发送完整历史"
            ))
            .with_code("codex_stateless_history_required"));
        }
    }
    if body.get("background").and_then(Value::as_bool) == Some(true) {
        return Err(GatewayError::bad_request(
            "Codex 订阅不支持后台生成，请使用普通或流式请求",
        ));
    }
    for key in [
        "max_output_tokens",
        "max_completion_tokens",
        "max_tokens",
        "temperature",
        "top_p",
        "previous_response_id",
        "conversation",
        "prompt",
        "background",
        "user",
        "safety_identifier",
        "prompt_cache_options",
    ] {
        body.remove(key);
    }
    body.retain(|key, _| !key.starts_with("_wb_"));
    if !body
        .get("service_tier")
        .and_then(Value::as_str)
        .is_some_and(|tier| matches!(tier, "priority" | "ultrafast"))
    {
        body.remove("service_tier");
    }
    for key in ["tools", "include"] {
        if body.get(key).is_some_and(|value| !value.is_array()) {
            return Err(GatewayError::bad_request(format!("Codex {key} 必须是数组")));
        }
    }
    if body
        .get("parallel_tool_calls")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(GatewayError::bad_request(
            "Codex parallel_tool_calls 必须是布尔值",
        ));
    }
    if body
        .get("prompt_cache_key")
        .is_some_and(|value| !value.is_string() && !value.is_null())
    {
        return Err(GatewayError::bad_request("Codex 会话缓存标识必须是文本"));
    }
    let input = match body.remove("input") {
        Some(Value::String(text)) => vec![
            json!({ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": text }] }),
        ],
        Some(Value::Array(items)) => items,
        _ => return Err(GatewayError::bad_request("Codex 请求缺少有效的 input 历史")),
    };
    for key in [
        "instructions",
        "tool_choice",
        "parallel_tool_calls",
        "include",
    ] {
        if body.get(key).is_some_and(Value::is_null) {
            body.remove(key);
        }
    }
    if body.get("tool_choice").and_then(Value::as_str) == Some("") {
        body.remove("tool_choice");
    }
    let input = normalize_input(input, native)?;
    if body
        .get("instructions")
        .is_some_and(|value| !value.is_string() && !value.is_null())
    {
        return Err(GatewayError::bad_request("Codex instructions 必须是文本"));
    }
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            catalog
                .as_ref()
                .and_then(|catalog| catalog.get("base_instructions"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
    if body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .is_none_or(|key| key.is_empty())
    {
        if let Some(key) = ["session-id", "thread-id", "session_id", "conversation_id"]
            .into_iter()
            .find_map(|key| {
                client_headers
                    .get(key)
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.is_empty())
            })
        {
            body.insert("prompt_cache_key".into(), json!(key));
        }
    }
    if body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .is_none_or(|key| key.is_empty())
    {
        if let Some(key) = conversation_key(&identity, &instructions, &input) {
            body.insert("prompt_cache_key".into(), json!(key));
        }
    }
    body.insert("instructions".into(), json!(instructions));
    body.insert("input".into(), Value::Array(input));
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    body.entry("tool_choice".to_string())
        .or_insert_with(|| json!("auto"));
    body.entry("parallel_tool_calls".to_string())
        .or_insert_with(|| json!(true));
    let effort = normalize_reasoning(&mut body, catalog.as_ref())?;
    declare_search(&mut body);
    validate_tool_names(&body)?;
    let mut headers = http::account_headers(&tokens.access, &tokens.account_id);
    headers.extend([
        ("Content-Type".into(), "application/json".into()),
        ("Accept".into(), "text/event-stream".into()),
        ("OpenAI-Beta".into(), "responses=experimental".into()),
        ("version".into(), http::version()),
    ]);
    for (key, value) in client_headers {
        let key = key.as_str();
        if !codex_header(key) || matches!(key, "version" | "session_id" | "conversation_id") {
            continue;
        }
        if let Ok(value) = value.to_str() {
            headers.push((key.to_string(), value.to_string()));
        }
    }
    if let Some(key) = body.get("prompt_cache_key").and_then(Value::as_str) {
        if key.len() > 512 || HeaderValue::from_str(key).is_err() {
            return Err(GatewayError::bad_request("Codex 会话缓存标识无效"));
        }
        headers.retain(|(name, _)| !matches!(name.as_str(), "session-id" | "thread-id"));
        headers.push(("session-id".into(), key.to_string()));
        headers.push(("thread-id".into(), key.to_string()));
    }
    let mut plan = ChatRequestPlan::chat(
        format!("{}/responses", http::BASE),
        headers,
        Value::Object(body),
    );
    plan.response = UpstreamResponse::Responses;
    plan.reasoning = Some(effort);
    plan.policy.respect_model_cooldown = true;
    Ok(plan)
}

fn normalize_input(input: Vec<Value>, native: bool) -> Result<Vec<Value>, GatewayError> {
    let mut output = Vec::with_capacity(input.len());
    for mut item in input {
        let Some(fields) = item.as_object_mut() else {
            return Err(GatewayError::bad_request("Codex input 的每一项必须是对象"));
        };
        let kind = fields
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message")
            .to_string();
        if kind == "item_reference" {
            return Err(
                GatewayError::bad_request("Codex 订阅不保存历史引用，请发送完整历史条目")
                    .with_code("codex_stateless_history_required"),
            );
        }
        if kind == "reasoning" && !native {
            // 跨协议的明文思考不是 Codex 的可重放密文，不能造一个 rs_* 让上游找。
            if fields
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_none_or(|value| value.is_empty())
            {
                continue;
            }
            fields.remove("content");
        }
        let notification = matches!(
            kind.as_str(),
            "function_call_output" | "custom_tool_call_output"
        ) && fields
            .get("call_id")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty());
        if !notification {
            fields.remove("id");
            if matches!(kind.as_str(), "message" | "reasoning") {
                fields.remove("status");
            }
        }
        if matches!(
            kind.as_str(),
            "function_call"
                | "function_call_output"
                | "local_shell_call"
                | "local_shell_call_output"
                | "custom_tool_call"
                | "custom_tool_call_output"
                | "tool_search_call"
                | "tool_search_output"
        ) {
            if let Some(id) = fields
                .get("call_id")
                .and_then(Value::as_str)
                .filter(|id| id.len() > 64)
            {
                fields.insert(
                    "call_id".into(),
                    json!(format!("{:x}", Sha256::digest(id.trim().as_bytes()))),
                );
            }
        }
        if kind == "message" && fields.get("role").and_then(Value::as_str) == Some("system") {
            fields.insert("role".into(), json!("developer"));
        }
        // 不丢失 orphan 工具结果，也不转成捏造的 assistant 文本；原生历史交给上游判定。
        fields.retain(|key, _| !key.starts_with("_wb_"));
        output.push(item);
    }
    Ok(output)
}

fn normalize_reasoning(
    body: &mut Map<String, Value>,
    catalog: Option<&Value>,
) -> Result<Option<String>, GatewayError> {
    let mut effort = None;
    if let Some(reasoning) = body.get_mut("reasoning").filter(|value| !value.is_null()) {
        let fields = reasoning
            .as_object_mut()
            .ok_or_else(|| GatewayError::bad_request("Codex reasoning 必须是对象"))?;
        if let Some(value) = fields.get("effort").and_then(Value::as_str) {
            let value = if value == "ultra" { "max" } else { value }.to_string();
            if let Some(levels) = catalog
                .and_then(|catalog| catalog.get("supported_reasoning_levels"))
                .and_then(Value::as_array)
            {
                if !levels.is_empty()
                    && !levels
                        .iter()
                        .any(|entry| account::text(entry, &["effort"]) == value)
                {
                    return Err(GatewayError::bad_request(
                        "这个 Codex 账号不支持所选思考等级，请换用相应套餐或调整等级",
                    )
                    .with_code("account_model_unavailable"));
                }
            }
            fields.insert("effort".into(), json!(value));
            effort = Some(value);
        }
        if effort.as_deref() != Some("none") {
            fields
                .entry("summary".to_string())
                .or_insert_with(|| json!("auto"));
        }
    }
    if effort.as_deref() != Some("none") {
        let include = body
            .entry("include".to_string())
            .or_insert_with(|| json!([]));
        let include = include
            .as_array_mut()
            .ok_or_else(|| GatewayError::bad_request("Codex include 必须是数组"))?;
        if !include
            .iter()
            .any(|value| value.as_str() == Some("reasoning.encrypted_content"))
        {
            include.push(json!("reasoning.encrypted_content"));
        }
    }
    if !body.contains_key("text")
        && catalog
            .and_then(|catalog| catalog.get("support_verbosity"))
            .and_then(Value::as_bool)
            == Some(true)
    {
        let verbosity = catalog
            .and_then(|catalog| catalog.get("default_verbosity"))
            .and_then(Value::as_str)
            .unwrap_or("medium");
        body.insert("text".into(), json!({ "verbosity": verbosity }));
    }
    Ok(effort)
}

fn conversation_key(identity: &str, instructions: &str, input: &[Value]) -> Option<String> {
    let mut hash = Sha256::new();
    hash.update(identity.as_bytes());
    hash.update([0]);
    hash.update(instructions.as_bytes());
    for item in input {
        hash.update(serde_json::to_vec(item).ok()?);
        if account::text(item, &["role"]) == "user" {
            let hex = format!("{:x}", hash.finalize());
            return Some(format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            ));
        }
    }
    None
}

fn codex_header(key: &str) -> bool {
    if key.contains("authorization")
        || matches!(
            key,
            "x-oai-attestation" | "x-openai-codex-luna-reserve" | "x-codex-turn-state"
        )
    {
        return false;
    }
    matches!(key, "session-id" | "thread-id" | "x-client-request-id")
        || key.starts_with("x-openai-")
        || key.starts_with("x-codex-")
}

fn declares_search(tools: &[Value]) -> bool {
    tools.iter().any(|tool| {
        let kind = account::text(tool, &["type"]);
        kind.starts_with("web_search")
            || kind == "function" && account::text(tool, &["name"]) == "web.run"
            || kind == "namespace"
                && account::text(tool, &["name"]) == "web"
                && tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .is_some_and(|tools| {
                        tools
                            .iter()
                            .any(|tool| account::text(tool, &["name"]) == "run")
                    })
    })
}

fn declare_search(body: &mut Map<String, Value>) {
    let Some(input) = body.get("input").and_then(Value::as_array) else {
        return;
    };
    if !input
        .iter()
        .any(|item| account::text(item, &["type"]) == "web_search_call")
    {
        return;
    }
    let additional = input
        .iter()
        .position(|item| account::text(item, &["type"]) == "additional_tools");
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if declares_search(&tools)
        || input.iter().any(|item| {
            account::text(item, &["type"]) == "additional_tools"
                && item
                    .get("tools")
                    .and_then(Value::as_array)
                    .is_some_and(|tools| declares_search(tools))
        })
    {
        return;
    }
    let offered = !tools.is_empty()
        || input.iter().any(|item| {
            account::text(item, &["type"]) == "additional_tools"
                && item
                    .get("tools")
                    .and_then(Value::as_array)
                    .is_some_and(|tools| !tools.is_empty())
        });
    let lite = body
        .get("reasoning")
        .and_then(|item| item.get("context"))
        .and_then(Value::as_str)
        == Some("all_turns");
    let search = json!({ "type": "web_search", "external_web_access": false });
    if let Some(index) = additional {
        if let Some(item) = body
            .get_mut("input")
            .and_then(Value::as_array_mut)
            .and_then(|items| items.get_mut(index))
        {
            let mut tools = item
                .get("tools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            tools.push(search);
            item["tools"] = Value::Array(tools);
        }
    } else if lite {
        if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
            input.insert(
                0,
                json!({ "type": "additional_tools", "role": "developer", "tools": [search] }),
            );
        }
    } else {
        let mut tools = tools;
        tools.push(search);
        body.insert("tools".into(), Value::Array(tools));
    }
    if !offered
        && body
            .get("tool_choice")
            .and_then(Value::as_str)
            .is_none_or(|choice| matches!(choice, "" | "auto" | "none"))
    {
        body.insert("tool_choice".into(), json!("none"));
    }
}

fn validate_tool_names(body: &Map<String, Value>) -> Result<(), GatewayError> {
    // Magpie 还原工具名需要回程映射；当前共用计划无此元数据，不能只改出站让工具失配。
    // 因此无效名字明确拒绝，合法工具名/namespace 与原生工具载荷不改。
    fn check(value: &Value, depth: usize) -> bool {
        if depth > 64 {
            return false;
        }
        if let Some(values) = value.as_array() {
            return values.iter().all(|value| check(value, depth + 1));
        }
        let kind = account::text(value, &["type"]);
        if matches!(
            kind,
            "function" | "custom" | "function_call" | "custom_tool_call"
        ) {
            let name = account::text(value, &["name"]);
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'.' | b'-'))
            {
                return false;
            }
        }
        value
            .get("tools")
            .is_none_or(|tools| check(tools, depth + 1))
    }
    for key in ["tools", "input", "tool_choice"] {
        if body.get(key).is_some_and(|value| !check(value, 0)) {
            return Err(GatewayError::bad_request(
                "Codex 工具名称只能包含英文字母、数字、下划线、点和连字符；未改名以免工具结果失配",
            ));
        }
    }
    Ok(())
}

pub(super) fn reasoning_patch(level: &str, model: &str) -> ReasoningPatch {
    if crate::server::core::model_rules::reasoning_is_off(level) {
        return ReasoningPatch::Skip {
            reason: "关闭思考的绑定保持客户端设置",
        };
    }
    let level = if level == "ultra" { "max" } else { level };
    let listed = models::list().iter().any(|entry| {
        account::text(entry, &["id"]) == model
            && entry
                .get("reasoningLevels")
                .and_then(Value::as_array)
                .is_some_and(|levels| levels.iter().any(|item| item.as_str() == Some(level)))
    });
    if listed {
        ReasoningPatch::Set {
            field: "reasoning_effort",
            value: json!(level),
        }
    } else {
        ReasoningPatch::Skip {
            reason: "Codex 账号目录尚未声明这个思考等级",
        }
    }
}
