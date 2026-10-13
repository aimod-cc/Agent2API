//! `/alpha/generate` 的请求构造：**8 键信封** + `params` 的逐字段改写 + 会话派生。
//!
//! ── 上游信封（键序与字段被参考实现的 `test/envelope.test.mjs` 锁定）──
//! ```text
//! config, memory, taste, skills, permissionMode, [threadId], mode, params
//! ```
//! `threadId` **仅当**会话标识是合法 UUID 时插入（非 UUID 整键省略）；
//! `memory` / `taste` / `skills` 恒为字面 `null`（不是空串、不是省略）。
//! 注：Rust 的 `serde_json` 默认按**字典序**序列化对象键（`Map` 是 BTreeMap），
//! 因此出网的键序与参考实现（JS 插入序）不同。JSON 对象的键序对上游没有语义，
//! 这里保留插入序的写法只为可读性。
//!
//! ── `config` 块：本地环境字段一律**稳定中性值**（重要）───────────
//! 真机发的是当前 cwd / 当天日期 / git 状态，反代发这些有两个问题：
//!   1. **暴露宿主**：真实目录会带上用户名与目录结构（规格 §4.3 的设备档案
//!      从指纹到 config 都是伪造值）；
//!   2. **破坏缓存**：会话（以及可能的 prompt 缓存）按请求前缀计算，
//!      逐请求变化的字段（cwd、git 状态、最近提交）会让同一场对话每次都换前缀。
//! 因此 `workingDir` 用固定伪造目录（与 `x-project-slug` 同源）、`structure` /
//! `recentCommits` 空数组、`isGitRepo` false、分支 / git 状态空串。
//! `date` 是**唯一**随时间变化的字段：真机就是当天日期，按 UTC 取
//! `YYYY-MM-DD`（与参考实现的 `toISOString().slice(0,10)` 同口径）。
//!
//! ── `params` 的契约（规格 §5.2，逐条照做）──────────────────────
//!   - `messages[*].content` **恒为块数组**（字符串会整条丢失）；
//!   - `system` 是**块数组**（非末块补 `\n`）；没有 system 时发**空格占位块**
//!     `[{type:"text",text:" "}]` —— 缺省会让上游注入约 7.5K token 的默认
//!     提示词并污染对话（规格坑 #3）；
//!   - `stream` 恒 `true`（上游只支持流；非流式由网关自己聚合）；
//!   - `max_tokens = min(请求值, 200000)`，缺省 64000；
//!   - `tools` **总是下发**（无工具时是空数组而不是缺键）、**没有 `type` 字段**、
//!     名字一个都不重写；
//!   - `tool_choice` 只有 `auto` / `any` / `tool`；`none` → 清空 tools 且
//!     **不下发** tool_choice（发 `none` 会被上游 400）；
//!   - 缓存断点：客户端已在任一 system / 消息块上打过 `cache_control` 就保留；
//!     否则若给了 OpenAI 的 `prompt_cache_key`，断点落在 **system 最后一块**。
//!
//! ── 会话与 threadId（规格 §5.4）────────────────────────────────
//! 优先级：`x-session-id` → `x-claude-code-session-id` → `session_id` →
//! `prompt_cache_key`（取值须 ≥ 8 字符）→ 都没有则**按请求内容派生**：
//! `sha1(key\0model\0system 各块文本\0首个非 user 消息之前的连续 user 文本\0)`
//! 前 16 字节、置 UUID 版本位后格式化成 UUID。派生结果恒为合法 UUID →
//! `threadId` 与 `x-session-id` 同值；客户端自带的非 UUID 值只进 header、
//! `threadId` 整键省略。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};

use super::endpoints;
use super::fingerprint;

/// 客户端没给模型时的默认值（参考实现的默认模型）
pub const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash";

/// `max_tokens` 缺省值（参考实现 `max_tokens || 64000`）
const DEFAULT_MAX_TOKENS: i64 = 64_000;

/// `max_tokens` 上限（参考实现 `Math.min(x, 200000)`）
const MAX_TOKENS_CAP: i64 = 200_000;

/// 会话标识的最小长度（参考实现的口径：短于 8 字符的值不算会话标识）
const MIN_SESSION_ID_LENGTH: usize = 8;

/// 一次构造好的请求（URL / 头 / 体）
pub struct BuiltRequest {
    /// 上游完整 URL
    pub url: String,
    /// 请求头（含 Authorization 与全部 CLI 伪装头）
    pub headers: Vec<(String, String)>,
    /// 已按本家规则改写的请求体（8 键信封）
    pub body: Value,
}

/// 构造 `POST /alpha/generate`（`api_key` 是账号的 `user_` key）。
pub fn build(api_key: &str, body: &Value, client_headers: &HeaderMap) -> BuiltRequest {
    let messages: Vec<Value> = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_MODEL)
        .to_string();

    // ── system 块（提到顶层 params.system，绝不留在 messages 里）──
    let mut system_blocks = extract_system_blocks(&messages);
    for index in 0..system_blocks.len().saturating_sub(1) {
        if let Some(text) = system_blocks
            .get_mut(index)
            .and_then(|block| block.get_mut("text"))
        {
            let appended = format!("{}\n", text.as_str().unwrap_or(""));
            *text = Value::String(appended);
        }
    }
    // 没有 system 时发空格占位（理由见模块头；这是硬要求，不是美化）
    if system_blocks.is_empty() {
        system_blocks.push(json!({ "type": "text", "text": " " }));
    }
    // 缓存断点：客户端打过就保留；否则 prompt_cache_key 落在 system 最后一块
    let prompt_cache_key = body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let cc_messages = convert_messages(&messages);

    // 会话标识：先看客户端头，都没有才按内容派生（派生需要 system 与 messages）
    let session_id = session_id_of(
        api_key,
        &model,
        &system_blocks,
        &messages,
        prompt_cache_key,
        client_headers,
    );
    let has_cache_marker = system_blocks.iter().any(has_cache_control)
        || cc_messages.iter().any(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|parts| parts.iter().any(has_cache_control))
        });
    if prompt_cache_key.is_some() && !has_cache_marker {
        if let Some(last) = system_blocks.last_mut() {
            if let Some(object) = last.as_object_mut() {
                object.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
            }
        }
    }

    // ── params ────────────────────────────────────────────────
    let mut params = Map::new();
    params.insert("model".to_string(), Value::String(model));
    params.insert("messages".to_string(), Value::Array(cc_messages));
    params.insert("max_tokens".to_string(), Value::from(max_tokens_of(body)));
    // CC 上游只支持流：非流式由网关自己缓冲 NDJSON 聚合（规格坑 #4）
    params.insert("stream".to_string(), Value::Bool(true));
    params.insert("system".to_string(), Value::Array(system_blocks));
    if let Some(temperature) = body.get("temperature").filter(|value| value.is_number()) {
        params.insert("temperature".to_string(), temperature.clone());
    }
    if let Some(effort) = body
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        params.insert(
            "reasoning_effort".to_string(),
            Value::String(effort.to_string()),
        );
    }
    // tools 总是下发（无工具时空数组，不是缺键）；没有 type 字段；名字不重写
    let mut tools = convert_tools(body);
    if let Some(choice) = body.get("tool_choice") {
        match tool_choice_of(choice) {
            ToolChoice::Disable => {
                // `none`：清空 tools 且不下发 tool_choice（发 none 会被上游 400）
                tools = Vec::new();
            }
            ToolChoice::Keep(value) => {
                params.insert("tool_choice".to_string(), value);
            }
        }
    }
    params.insert("tools".to_string(), Value::Array(tools));
    if let Some(parallel) = body.get("parallel_tool_calls").and_then(Value::as_bool) {
        params.insert("parallel_tool_calls".to_string(), Value::Bool(parallel));
    }

    // ── 信封（键序见模块头；threadId 仅合法 UUID 时插入）──────────
    let mut envelope = Map::new();
    envelope.insert("config".to_string(), config_block());
    envelope.insert("memory".to_string(), Value::Null);
    envelope.insert("taste".to_string(), Value::Null);
    envelope.insert("skills".to_string(), Value::Null);
    envelope.insert(
        "permissionMode".to_string(),
        Value::String("standard".to_string()),
    );
    if is_wire_uuid(&session_id) {
        envelope.insert("threadId".to_string(), Value::String(session_id.clone()));
    }
    envelope.insert("mode".to_string(), Value::String("agent".to_string()));
    envelope.insert("params".to_string(), Value::Object(params));

    // ── 请求头（规格 §3.1 的逐字集合与顺序）────────────────────
    let mut headers: Vec<(String, String)> = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        (
            "User-Agent".to_string(),
            endpoints::CLI_USER_AGENT.to_string(),
        ),
        (
            "x-command-code-version".to_string(),
            endpoints::protocol_version(),
        ),
        (
            "x-cli-environment".to_string(),
            endpoints::cli_environment(),
        ),
        (
            "x-project-slug".to_string(),
            fingerprint::slugify_project_path(&fingerprint::device_project_dir()),
        ),
        // 字面量字符串 "false"，不是布尔 false
        ("x-taste-learning".to_string(), "false".to_string()),
        ("x-session-id".to_string(), session_id),
        (
            "Authorization".to_string(),
            format!("Bearer {}", api_key.trim()),
        ),
        ("traceparent".to_string(), fingerprint::traceparent()),
    ];
    if let Some(zdr) = endpoints::zdr_header(client_headers) {
        headers.push(zdr);
    }

    BuiltRequest {
        url: endpoints::url(endpoints::GENERATE_PATH),
        headers,
        body: Value::Object(envelope),
    }
}

/// `config` 块：本地环境字段全部稳定中性值（理由见模块头）
fn config_block() -> Value {
    json!({
        "workingDir": fingerprint::device_project_dir(),
        "date": utc_date(),
        "environment": fingerprint::DEVICE_PLATFORM,
        "structure": [],
        "isGitRepo": false,
        "currentBranch": "",
        "mainBranch": "",
        "gitStatus": "",
        "recentCommits": [],
    })
}

/// 当天 UTC 日期（`YYYY-MM-DD`，对齐参考实现的 `toISOString().slice(0,10)`）
fn utc_date() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// 一个块上是否带缓存断点
fn has_cache_control(block: &Value) -> bool {
    block
        .get("cache_control")
        .map(crate::server::core::protocol::is_truthy)
        .unwrap_or(false)
}

/// 从 chat 消息里提系统提示（`system` / `developer` 都算）→ 块数组。
///
/// 形态对齐 CLI 的 `toWireSystem`：块数组、逐块保留 `cache_control`；
/// 空文本块（且无断点）丢弃 —— 上游对一个空 text 块没有兴趣。
fn extract_system_blocks(messages: &[Value]) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "system" && role != "developer" {
            continue;
        }
        match message.get("content") {
            Some(Value::String(text)) => {
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
            }
            Some(Value::Array(parts)) => {
                for part in parts {
                    // `text ?? content ?? ""`（两者都认：Anthropic 风格用 content）
                    let text = part
                        .get("text")
                        .filter(|value| !value.is_null())
                        .or_else(|| part.get("content").filter(|value| !value.is_null()))
                        .map(crate::server::core::protocol::string_value)
                        .unwrap_or_default();
                    let cache_control = part
                        .get("cache_control")
                        .filter(|value| crate::server::core::protocol::is_truthy(value))
                        .cloned();
                    if text.is_empty() && cache_control.is_none() {
                        continue;
                    }
                    let mut block = Map::new();
                    block.insert("type".to_string(), Value::String("text".to_string()));
                    block.insert("text".to_string(), Value::String(text));
                    if let Some(cache_control) = cache_control {
                        block.insert("cache_control".to_string(), cache_control);
                    }
                    blocks.push(Value::Object(block));
                }
            }
            Some(other) if !other.is_null() => {
                blocks.push(json!({
                    "type": "text",
                    "text": crate::server::core::protocol::string_value(other),
                }));
            }
            _ => {}
        }
    }
    blocks
}

/// 非 system 消息 → CC wire 形态（`content` 恒为块数组）
fn convert_messages(messages: &[Value]) -> Vec<Value> {
    // tool_call_id → 工具名 的反查表（tool-result 要回填 toolName；不重命名）
    let mut tool_names: Map<String, Value> = Map::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "assistant" {
            continue;
        }
        let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for call in calls {
            let Some(id) = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            tool_names.insert(id.to_string(), Value::String(name.to_string()));
        }
    }
    let mut out: Vec<Value> = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "system" || role == "developer" {
            continue;
        }
        let content = match role {
            "user" => user_content(message.get("content")),
            "assistant" => assistant_content(message),
            "tool" => {
                let call_id = message
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = tool_names
                    .get(&call_id)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        message
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                vec![json!({
                    "type": "tool-result",
                    "toolCallId": call_id,
                    "toolName": name,
                    "output": {
                        "type": "text",
                        "value": tool_output_value(message.get("content")),
                    },
                })]
            }
            // 未知 role 兜底：归一成 user（CC 对未知 role 会整条拒）
            _ => {
                let text = message
                    .get("content")
                    .map(crate::server::core::protocol::string_value)
                    .unwrap_or_default();
                vec![json!({ "type": "text", "text": text })]
            }
        };
        out.push(json!({ "role": role_or_user(role), "content": content }));
    }
    out
}

/// 未知 role 一律折成 `user`（CC 的 role 词表只有 user / assistant / tool）
fn role_or_user(role: &str) -> &str {
    match role {
        "user" | "assistant" | "tool" => role,
        _ => "user",
    }
}

/// user 消息内容 → 块数组（字符串转单个 text 块；`image_url` → CC 的 image 块）
fn user_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({ "type": "text", "text": text })],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| !part.is_null())
            .map(|part| {
                if part.get("type").and_then(Value::as_str) == Some("image_url") {
                    let url = part
                        .pointer("/image_url/url")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let mut block = Map::new();
                    block.insert("type".to_string(), Value::String("image".to_string()));
                    block.insert("image".to_string(), Value::String(url.to_string()));
                    if let Some(mime) = data_url_mime(url) {
                        block.insert("mimeType".to_string(), Value::String(mime));
                    }
                    return Value::Object(block);
                }
                part.clone()
            })
            .collect(),
        Some(other) if !other.is_null() => {
            vec![
                json!({ "type": "text", "text": crate::server::core::protocol::string_value(other) }),
            ]
        }
        _ => Vec::new(),
    }
}

/// 从 `data:<mime>;base64,…` 里取 mime（不是 data URL 时 None）
fn data_url_mime(url: &str) -> Option<String> {
    let rest = url.strip_prefix("data:")?;
    let end = rest.find([';', ','])?;
    let mime = rest[..end].trim();
    (!mime.is_empty()).then(|| mime.to_string())
}

/// assistant 消息内容 → 块数组。
///
/// 次序是硬要求：`[reasoning, text, tool-call]` —— CC 在思考模式下会校验
/// reasoning 是否随历史回传（规格坑 #14），丢弃或换序会让多轮直接失败。
fn assistant_content(message: &Value) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    let reasoning_content = message
        .get("reasoning_content")
        .filter(|value| crate::server::core::protocol::is_truthy(value))
        .map(crate::server::core::protocol::string_value)
        .filter(|text| !text.is_empty());
    if let Some(text) = &reasoning_content {
        parts.push(json!({ "type": "reasoning", "text": text }));
    }
    match message.get("content") {
        Some(Value::String(text)) => {
            if !text.is_empty() {
                parts.push(json!({ "type": "text", "text": text }));
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                if item.is_null() {
                    continue;
                }
                match item.get("type").and_then(Value::as_str) {
                    // text 块原样保留（含 cache_control 等客户端标记）
                    Some("text") => parts.push(item.clone()),
                    // 客户端直接把 reasoning 放进 content 数组时同样透传；
                    // 已有 reasoning_content 字段则不重复
                    Some("reasoning") if reasoning_content.is_none() => parts.push(item.clone()),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let call_id = call.get("id").and_then(Value::as_str).unwrap_or("");
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let input = match call.pointer("/function/arguments") {
                Some(Value::String(text)) => serde_json::from_str::<Value>(text)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({})),
                Some(value) if value.is_object() => value.clone(),
                _ => json!({}),
            };
            parts.push(json!({
                "type": "tool-call",
                "toolCallId": call_id,
                "toolName": name,
                "input": input,
            }));
        }
    }
    parts
}

/// CLI 的 `toWireToolOutput`：只取文本块、用 `\n` 拼接
fn tool_output_value(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => crate::server::core::protocol::string_value(other),
        None => String::new(),
    }
}

/// 工具的 wire 形态：只有 `name` / `description` / `input_schema`（**没有 `type`**）
fn convert_tools(body: &Value) -> Vec<Value> {
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return Vec::new();
    };
    tools
        .iter()
        .map(|tool| {
            let function = tool.get("function");
            let name = function
                .and_then(|value| value.get("name"))
                .or_else(|| tool.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let description = function
                .and_then(|value| value.get("description"))
                .or_else(|| tool.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let input_schema = function
                .and_then(|value| value.get("parameters"))
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            json!({
                "name": name,
                "description": description,
                "input_schema": input_schema,
            })
        })
        .collect()
}

/// `tool_choice` 的处置结果
enum ToolChoice {
    /// `none`：清空 tools 且不下发 tool_choice
    Disable,
    /// 原样（已翻译）下发
    Keep(Value),
}

/// chat 的 `tool_choice` → CC 的 `tool_choice`（枚举只有 auto / any / tool）
fn tool_choice_of(choice: &Value) -> ToolChoice {
    match choice {
        Value::String(text) => match text.trim() {
            "none" => ToolChoice::Disable,
            // `required` → `any`；其余（含 `auto`）一律 auto —— 与参考实现同款
            "required" => ToolChoice::Keep(json!({ "type": "any" })),
            _ => ToolChoice::Keep(json!({ "type": "auto" })),
        },
        Value::Object(object) => {
            let kind = object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            match kind {
                "none" => ToolChoice::Disable,
                "function" => {
                    let name = object
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    ToolChoice::Keep(json!({ "type": "tool", "name": name }))
                }
                // 已是 CC 形态（auto / any / tool）或未知对象：原样下发
                _ => ToolChoice::Keep(choice.clone()),
            }
        }
        _ => ToolChoice::Keep(json!({ "type": "auto" })),
    }
}

/// `max_tokens`：`min(请求值 || 64000, 200000)`（`max_completion_tokens` 作为别名）
fn max_tokens_of(body: &Value) -> i64 {
    let requested = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|number| number as i64))
        })
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    requested.min(MAX_TOKENS_CAP)
}

/// 会话标识：客户端头优先，都没有则按内容派生（见模块头）
fn session_id_of(
    api_key: &str,
    model: &str,
    system_blocks: &[Value],
    messages: &[Value],
    prompt_cache_key: Option<&str>,
    client_headers: &HeaderMap,
) -> String {
    let from_header = |name: &str| -> Option<String> {
        client_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| value.chars().count() >= MIN_SESSION_ID_LENGTH)
            .map(str::to_string)
    };
    if let Some(id) = from_header("x-session-id") {
        return id;
    }
    if let Some(id) = from_header("x-claude-code-session-id") {
        return id;
    }
    if let Some(id) = from_header("session_id") {
        return id;
    }
    if let Some(id) =
        prompt_cache_key.filter(|value| value.chars().count() >= MIN_SESSION_ID_LENGTH)
    {
        return id.to_string();
    }
    derive_session_id(api_key, model, system_blocks, messages)
}

/// 按请求内容派生 UUID（`sha1` 逐块 `\0` 分隔，前 16 字节置 UUID 版本位）。
///
/// 只吃「首个非 user 消息之前的连续 user 文本」：多轮对话的历史会不断增长，
/// 取首轮才能让同一场对话一直映射到同一个 id（长会话的缓存前缀因此稳定）。
fn derive_session_id(
    api_key: &str,
    model: &str,
    system_blocks: &[Value],
    messages: &[Value],
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(api_key.as_bytes());
    hasher.update([0u8]);
    hasher.update(model.as_bytes());
    hasher.update([0u8]);
    for block in system_blocks {
        if let Some(text) = block.get("text").and_then(Value::as_str) {
            hasher.update(text.as_bytes());
            hasher.update([0u8]);
        }
    }
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "user" {
            break;
        }
        let Some(parts) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in parts {
            if part.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                hasher.update(text.as_bytes());
                hasher.update([0u8]);
            }
        }
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // 置版本位（v4）与变体位，让它恒为合法 UUID 形态
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// 合法 UUID 形态（大小写不敏感；`threadId` 只接受这一种）
fn is_wire_uuid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let lengths = [8usize, 4, 4, 4, 12];
    parts
        .iter()
        .zip(lengths)
        .all(|(part, length)| part.len() == length && part.chars().all(|ch| ch.is_ascii_hexdigit()))
}
