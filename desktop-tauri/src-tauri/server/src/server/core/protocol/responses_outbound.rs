//! Responses 协议的**出站**转换：chat 请求 → Responses 上游请求、
//! 上游 Responses SSE → chat SSE（自定义提供商转发的 `responses` 分支）。
//!
//! ── 与 `responses.rs` 的方向关系 ─────────────────────────────
//! `responses.rs` 服务的是**下游入口**（`api::protocol` 的 `/v1/responses`）：
//! 把客户端发来的 Responses 请求翻成 chat、把回程的 chat 响应/SSE 翻回
//! Responses。本文件是它的**反方向**，服务自定义提供商的**上游**：
//! 转发层手里的统一货币是 chat，但这家上游说 Responses，于是
//!   - 请求：`responses_request_from_chat`（chat 体 → Responses 体，逐字段
//!     对着 `chat_from_responses` 反推）；
//!   - 响应：`ChatFromResponsesStream`（上游 Responses SSE → 标准 chat SSE，
//!     之后照走既有的 `ForwardStream` / 聚合器 —— 那两层只认 chat 帧）。
//!
//! ── 上游恒以 `stream:true` 被请求 ────────────────────────────
//! 与 chat 协议同一条策略（各家上游的流式才是完整能力；非流式下游由网关
//! 收流聚合）。因此响应侧只需要流式转换器；非流式 JSON 路径不会有上游
//! 来源。事件流的折法参考 OmniProxy 的 `responsesStreamToChat`，字段口径
//! 按本项目 chat 侧（`responses.rs` 回程消费的形状）收敛。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 同 `mod.rs`：零 unwrap/expect/panic。请求转换不变；响应严格校验由
//! `ChatFromResponsesStream::new_strict` 显式启用，既有 `new` 仍保留兼容模式。

use serde_json::{json, Map, Value};

use super::{
    content_parts, content_text, is_truthy, json_text, native_tool,
    random_id, string_field, string_value, FIELD_ENCRYPTED_CONTENT,
};
use super::responses::{image_url_of, tool_output_parts, ConvertError, ToolOutput};
use super::tool_plan;

// ─── 请求：Chat → Responses ─────────────────────────────────

/// Chat Completions 请求体 → Responses 请求体。
///
/// ── 逐字段对着 [`super::responses::chat_from_responses`] 反推 ────
/// 那份转换把 Responses 请求折成 chat，字段映射全在里面；这里逐条走它的
/// 完整清单（漏一条的症状是「客户端参数静默失效」）：
///   - `model`（由调用方给上游真名，见 `providers::custom::forward`）
///   - `messages` → `instructions`（system 消息）+ `input`（message /
///     function_call / function_call_output 项）
///   - `stream` / `max_output_tokens`（← `max_completion_tokens` / `max_tokens`）
///   - `temperature` / `top_p` / `service_tier` / `parallel_tool_calls` /
///     `user` / `metadata`
///   - `tools`（chat 嵌套 function → Responses 扁平 function）
///   - `tool_choice`、`response_format` → `text.format`
///   - `reasoning_effort` → `reasoning:{effort, summary:"auto"}`
///   - `store: false`（本网关无状态；对上游同样不存）
///
/// 不支持的 chat 字段（`n` / `logprobs` 等）**原样丢弃**：Responses 没有
/// 对应形态，发送上游不认识的键只会换来一次 400。
pub fn responses_request_from_chat(chat: &Value, model: &str) -> Result<Value, ConvertError> {
    let Some(messages) = chat.get("messages").and_then(Value::as_array) else {
        return Err("缺少 messages 数组".to_string());
    };
    let mut instructions: Vec<String> = Vec::new();
    let mut items: Vec<Value> = Vec::new();
    for message in messages {
        let role = {
            let raw = string_field(message, "role").to_lowercase();
            if raw == "developer" {
                "system".to_string()
            } else if raw.is_empty() {
                "user".to_string()
            } else {
                raw
            }
        };
        match role.as_str() {
            // system / developer → 顶层 instructions（多条用空行拼接，
            // 与 anthropic 侧 system_text 的拼接口径一致）
            "system" => {
                let text = content_text(message.get("content").unwrap_or(&Value::Null));
                if !text.trim().is_empty() {
                    instructions.push(text);
                }
            }
            // tool 结果 → function_call_output 项（call_id 是配对的钥匙，
            // 缺了它上游在多轮里认不出这条结果属于哪次调用）
            "tool" => {
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": string_field(message, "tool_call_id"),
                    "output": tool_output_to_responses(tool_output_parts(message.get("content"))),
                }));
            }
            "assistant" => push_assistant_items(&mut items, message),
            // user 与「认不出的角色」都当 user（与 chat_from_responses 的
            // 兜底同一取向：能跑比报错好）
            _ => {
                let content =
                    chat_content_to_responses(message.get("content").unwrap_or(&Value::Null), "user");
                items.push(json!({ "type": "message", "role": "user", "content": content }));
            }
        }
    }

    let mut out = Map::new();
    out.insert("model".to_string(), Value::String(model.to_string()));
    out.insert("input".to_string(), Value::Array(items));
    out.insert(
        "stream".to_string(),
        Value::Bool(chat.get("stream").and_then(Value::as_bool).unwrap_or(false)),
    );
    if !instructions.is_empty() {
        out.insert(
            "instructions".to_string(),
            Value::String(instructions.join("\n\n")),
        );
    }
    // max_output_tokens ← max_completion_tokens（Responses 口径的字段名），
    // 退到 max_tokens；非正数视为没给（0 / 负数发上去只会被上游拒）
    let max_output = ["max_completion_tokens", "max_tokens"]
        .iter()
        .find_map(|key| chat.get(*key).and_then(Value::as_i64).filter(|value| *value > 0));
    if let Some(value) = max_output {
        out.insert("max_output_tokens".to_string(), Value::from(value));
    }
    for key in ["temperature", "top_p", "service_tier", "parallel_tool_calls", "user", "metadata"] {
        if let Some(value) = chat.get(key).filter(|value| !value.is_null()) {
            out.insert(key.to_string(), value.clone());
        }
    }
    // 工具声明：函数工具翻译成 Responses 扁平形态；原生（服务端执行）声明只有
    // 「来源就是 Responses」的原样恢复（保真，含 web_search / tool_search 等
    // 宿主工具的一切字段），跨协议的不猜 —— 剔除并留痕（见 `native_tool` 模块头）
    let mut natives: Vec<Value> = Vec::new();
    if let Some(tools) = chat.get("tools").and_then(Value::as_array) {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(|tool| tool_to_responses(tool, &mut natives))
            .collect();
        if !converted.is_empty() {
            out.insert("tools".to_string(), Value::Array(converted));
        }
    }
    let mut choice_reason: Option<String> = None;
    if let Some(choice) = chat.get("tool_choice").filter(|value| is_truthy(value)) {
        // 点名的工具被剔除时 `tool_choice` 一并撤掉（理由同 `anthropic_outbound`）
        match native_tool::choice_conflict(choice, &natives) {
            Some(reason) => choice_reason = Some(reason),
            None => {
                out.insert("tool_choice".to_string(), tool_choice_to_responses(choice));
            }
        }
    }
    if !natives.is_empty() || choice_reason.is_some() {
        let dropped = native_tool::Downgrade { tools: natives, choice: choice_reason };
        crate::server::logging::log(
            "[Responses]",
            &dropped.describe(None, Some("目标上游按 responses 协议收，只认 responses 来源的原生声明")),
        );
    }
    // response_format → text.format（json_schema 的嵌套 → 扁平）
    if let Some(format) = chat.get("response_format").filter(|value| is_truthy(value)) {
        out.insert("text".to_string(), json!({ "format": format_to_responses(format) }));
    }
    // reasoning_effort → reasoning.effort（本网关各上游的「思考档位」统一
    // 从这里进；summary:"auto" 与参考实现同款 —— 让上游回思考摘要）
    if let Some(effort) = chat
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        out.insert(
            "reasoning".to_string(),
            json!({ "effort": effort, "summary": "auto" }),
        );
    }
    // store:false 与模块头同一理由（网关无状态，也不许上游存）
    out.insert("store".to_string(), Value::Bool(false));
    Ok(Value::Object(out))
}

/// 一条 assistant 消息 → 零到多个 input 项（顺序：reasoning → message →
/// function_call）。
///
/// 顺序不是随意的：回程的 `chat_from_responses` 按「reasoning 项在前、
/// 本轮 assistant 项在后」消费（`PendingReasoning`），反向也必须产出同一
/// 形状，多轮历史才能被上游正确读取。
fn push_assistant_items(items: &mut Vec<Value>, message: &Value) {
    let reasoning = {
        let from_field = string_field(message, "reasoning_content");
        if from_field.is_empty() {
            string_field(message, "reasoning")
        } else {
            from_field
        }
    };
    // 加密连续性载体（store=false 多轮）恢复进 reasoning 项：与 summary 文本
    // 可以并存（OpenAI 官方回传的形态就是两者同项），只有加密体时 summary
    // 给空数组。没有这个恢复，Codex / Grok CLI 的下一轮就接不上推理链 ——
    // 它们靠把上游回传的加密体原样带回下一轮请求来延续推理。
    let encrypted = message
        .get(FIELD_ENCRYPTED_CONTENT)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty());
    if !reasoning.is_empty() || encrypted.is_some() {
        let mut item = json!({
            "type": "reasoning",
            "id": random_id("rs"),
            "summary": if reasoning.is_empty() {
                Vec::<Value>::new()
            } else {
                vec![json!({ "type": "summary_text", "text": reasoning })]
            },
        });
        if let Some(encrypted) = encrypted {
            if let Some(object) = item.as_object_mut() {
                object.insert(
                    "encrypted_content".to_string(),
                    Value::String(encrypted.to_string()),
                );
            }
        }
        items.push(item);
    }
    // 正文：Responses 不需要空的 assistant message（只带工具调用的轮次
    // 只发 function_call 项）；空正文跳过也能避免上游把空消息判成错误
    let text = content_text(message.get("content").unwrap_or(&Value::Null));
    if !text.is_empty() {
        items.push(json!({
            "type": "message",
            "id": random_id("msg"),
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text }],
        }));
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let call_id = {
                let raw = string_field(call, "id");
                if raw.is_empty() {
                    random_id("call")
                } else {
                    raw
                }
            };
            let name = call
                .pointer("/function/name")
                .map(string_value)
                .unwrap_or_default();
            let arguments = {
                let raw = json_text(call.pointer("/function/arguments").unwrap_or(&Value::Null));
                if raw.is_empty() {
                    "{}".to_string()
                } else {
                    raw
                }
            };
            items.push(json!({
                "type": "function_call",
                "id": random_id("fc"),
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
            }));
        }
    }
}

/// 拆开的工具输出 → Responses 的 `function_call_output.output`。
///
/// ── 与入站方向相反的取舍（不是笔误）──────────────────────────
/// 入站（`responses.rs`）图片**必须**挪出 tool 消息：Chat 不许 `tool` 角色带
/// 图片（OpenAI 直接 400，见 `responses::PendingImages`）。出站这边不用 ——
/// Responses 官方 schema 里 `output` 本来就是
/// `String | Array[ResponseInputText | ResponseInputImage | …]`，图表留在工具结果
/// 里才是保真形态（DeepSeek 的 Responses 文档同样声明 `input_image` 按真图片
/// 处理）。所以这边只是「别把它拍平成 JSON 文本」，不搬消息。
///
/// 没有图片时保持字符串形态：字符串对各家上游最友好，也是原来就在发的形状。
fn tool_output_to_responses(output: ToolOutput) -> Value {
    if output.images.is_empty() {
        return Value::String(output.text);
    }
    let mut parts: Vec<Value> = Vec::new();
    if !output.text.is_empty() {
        parts.push(json!({ "type": "input_text", "text": output.text }));
    }
    for image in &output.images {
        let url = image_url_of(image);
        if !url.is_empty() {
            parts.push(json!({ "type": "input_image", "image_url": url }));
        }
    }
    if parts.is_empty() {
        // 图片块全都取不到 url（只剩 file_id 之类）：退回文本，别发空数组上去
        return Value::String(output.text);
    }
    Value::Array(parts)
}

/// Chat content（字符串或块数组）→ Responses 的 input content。
///
/// 口径对着 `chat_from_responses` 的 `content_to_chat` 反推：
///   - 纯文本（字符串 / 全文本块）保持该有的形态（user 可用字符串，
///     assistant 用 `[{type:"output_text"}]`）；
///   - 图片块 → `input_image`（url 取 `image_url` 的字符串或 `{url}` 形态）；
///   - 其余块（音频/文件等）**丢弃** —— 它们在 chat 侧本来就没有来源，
///     透传一个上游不认识的类型只会让整条请求 400。
fn chat_content_to_responses(content: &Value, role: &str) -> Value {
    let text_kind = if role == "assistant" { "output_text" } else { "input_text" };
    if let Some(text) = content.as_str() {
        if role == "assistant" {
            return if text.is_empty() {
                Value::String(String::new())
            } else {
                json!([{ "type": "output_text", "text": text }])
            };
        }
        return Value::String(text.to_string());
    }
    let parts = content_parts(content);
    if parts.is_empty() {
        return Value::String(String::new());
    }
    let mut out: Vec<Value> = Vec::new();
    for part in parts {
        if let Some(text) = part.as_str() {
            out.push(json!({ "type": text_kind, "text": text }));
            continue;
        }
        let kind = string_field(part, "type").to_lowercase();
        match kind.as_str() {
            "text" | "input_text" | "output_text" => {
                out.push(json!({ "type": text_kind, "text": string_field(part, "text") }));
            }
            "image_url" | "input_image" if role == "user" => {
                let url = image_url_of(part);
                if !url.is_empty() {
                    out.push(json!({ "type": "input_image", "image_url": url }));
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        Value::String(String::new())
    } else {
        Value::Array(out)
    }
}

/// Chat 工具声明 → Responses 工具声明（扁平）。
///
/// `parameters` 缺省给空对象 schema（`nested_from_flat` 的同一兜底）；
/// `strict` 原样搬运（两协议同名）。
///
/// 原生（服务端执行）声明：来源是 Responses 的原样恢复（`natives` 不收，
/// 保真）；其余（Anthropic 来源、或 chat 入口的方言原生工具）收进 `natives`
/// 由调用方留痕剔除 —— 目标协议是 responses，别的原生类型没有对应形态。
fn tool_to_responses(tool: &Value, natives: &mut Vec<Value>) -> Option<Value> {
    if native_tool::is_native(tool) {
        if native_tool::origin_of(tool) == Some(native_tool::ORIGIN_RESPONSES) {
            return Some(native_tool::restore(tool));
        }
        natives.push(tool.clone());
        return None;
    }
    if !tool_plan::is_nested_function(tool) {
        // 非嵌套形态（裸 name/parameters）：出站没有可靠的翻译口径，
        // 丢弃而不是发一个半成品上去
        return None;
    }
    let function = tool.get("function")?;
    let name = string_field(function, "name");
    if name.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String("function".to_string()));
    out.insert("name".to_string(), Value::String(name));
    if let Some(description) = function.get("description").filter(|value| is_truthy(value)) {
        out.insert("description".to_string(), description.clone());
    }
    let parameters = function
        .get("parameters")
        .filter(|value| is_truthy(value))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    out.insert("parameters".to_string(), parameters);
    if let Some(strict) = function.get("strict") {
        out.insert("strict".to_string(), strict.clone());
    }
    Some(Value::Object(out))
}

/// Chat 的 tool_choice → Responses 的 tool_choice
/// （`{type:"function", function:{name}}` → `{type:"function", name}`；
/// 字符串形态两协议同名，原样保留）。
fn tool_choice_to_responses(choice: &Value) -> Value {
    if choice.is_string() {
        return choice.clone();
    }
    let name = {
        let nested = choice.pointer("/function/name").map(string_value).unwrap_or_default();
        if nested.is_empty() {
            string_field(choice, "name")
        } else {
            nested
        }
    };
    if name.is_empty() {
        return choice.clone();
    }
    json!({ "type": "function", "name": name })
}

/// Chat 的 `response_format` → Responses 的 `text.format`
/// （`{type:"json_schema", json_schema:{…}}` → `{type:"json_schema", …}`）。
fn format_to_responses(format: &Value) -> Value {
    let kind = string_field(format, "type");
    if kind != "json_schema" {
        return format.clone();
    }
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String("json_schema".to_string()));
    for key in ["name", "description", "schema", "strict"] {
        if let Some(value) = format
            .pointer(&format!("/json_schema/{key}"))
            .filter(|value| !value.is_null())
        {
            out.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(out)
}

// ─── 响应：Responses SSE → Chat SSE（流式）────────────────────

mod stream;
pub use stream::ChatFromResponsesStream;
