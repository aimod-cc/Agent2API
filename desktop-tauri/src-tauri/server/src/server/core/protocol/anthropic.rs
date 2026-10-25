//! Anthropic Messages API（`POST /v1/messages`）↔ Chat Completions 的双向转换。
//!
//! ── 与 Responses 模块的关键结构差异 ──────────────────────────
//! Anthropic 与 Chat 的差异比 Responses 更大，有三处必须专门处理：
//!
//!   1. **system 是顶层字段**（不是 messages 里的一条）—— Chat 侧要把它变成
//!      一条 `role:"system"` 消息放在最前。
//!   2. **工具结果在 user 消息里**（`{type:"tool_result"}` 内容块），而 Chat 侧
//!      是独立的 `role:"tool"` 消息。转换时要把它们拆出来。
//!   3. **工具调用是 content 块**（`{type:"tool_use"}`），Chat 侧是
//!      `message.tool_calls` 数组。且 Anthropic **要求** tool_use 与 tool_result
//!      成对出现，所以回程要保证配对（见 `normalize_tool_pairing`）。
//!
//! ── 思考（thinking）怎么映射 ────────────────────────────────
//! Anthropic 侧是 `thinking` 内容块 + `signature`（签名，用于多轮校验）；
//! Chat 侧是 `reasoning_content` 字符串。Chat 没有签名这个概念，所以：
//!   - 回程（Anthropic → Chat）：thinking 块折进 `reasoning_content`，
//!     signature **丢弃**（Chat 侧无处安放，且下游上游都不校验它）；
//!   - 去程（Chat → Anthropic）：`reasoning_content` 折成 thinking 块，
//!     不带 signature（Anthropic 的签名只在它自己的多轮里需要）。
//!
//! `thinking.type:"enabled"` 时 Anthropic 要求 `budget_tokens < max_tokens`，
//! 所以去程会把 max_tokens 抬高到 budget 之上（参考实现同款处理）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! 同 `mod.rs`：零 unwrap/expect/panic。

use serde_json::{json, Map, Value};

use super::{
    content_parts, content_text, event_frame, is_truthy, json_text, native_tool, random_id,
    string_field, string_value, tool_plan, SseLineBuffer, TOOL_IMAGE_PLACEHOLDER,
    FIELD_CACHE_CONTROL, FIELD_IS_ERROR,
};
use super::responses::ConvertError;
use crate::server::logging;

/// Anthropic 的 `max_tokens` 缺省值。
///
/// Anthropic 官方**要求**这个字段必填；客户端漏填时给一个保守值而不是报错
/// （报错会让「只差一个字段」的请求整个失败，而 8192 对绝大多数对话够用）。
///
/// `pub(super)`：出站方向（`anthropic_outbound`）补同一个缺省值 —— 两处
/// 各定义一份迟早漂移。
pub(super) const DEFAULT_MAX_TOKENS: i64 = 8192;

// ─── 请求：Anthropic → Chat ─────────────────────────────────

/// Anthropic Messages 请求体 → Chat Completions 请求体。
pub fn chat_from_anthropic(body: &Value) -> Result<Value, ConvertError> {
    let mut out = Map::new();
    out.insert("model".to_string(), body.get("model").cloned().unwrap_or(Value::Null));

    let mut messages: Vec<Value> = Vec::new();
    // ① system 顶层字段 → 最前的一条 system 消息
    if let Some(system) = body.get("system") {
        let text = system_text(system);
        if !text.trim().is_empty() {
            let mut entry = Map::new();
            entry.insert("role".to_string(), Value::String("system".to_string()));
            entry.insert("content".to_string(), Value::String(text));
            // system 的缓存断点（Claude Code 打在 system 最后一个块上）同样
            // 走消息级暂存，出站时落回 system 块
            if let Some(cache) = system
                .as_array()
                .map(|parts| last_cache_control(parts))
                .unwrap_or(None)
            {
                entry.insert(FIELD_CACHE_CONTROL.to_string(), cache);
            }
            messages.push(Value::Object(entry));
        }
    }
    // ② 逐条消息转换
    let Some(input) = body.get("messages").and_then(Value::as_array) else {
        return Err("缺少 messages 数组".to_string());
    };
    for message in input {
        convert_message(&mut messages, message)?;
    }

    out.insert("messages".to_string(), Value::Array(messages));
    out.insert(
        "stream".to_string(),
        Value::Bool(body.get("stream").and_then(Value::as_bool).unwrap_or(false)),
    );
    // max_tokens 是 Anthropic 的必填项；Chat 侧用 max_tokens（部分上游也认
    // max_completion_tokens，这里给兼容性最好的那个）
    let max_tokens = body
        .get("max_tokens")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    out.insert("max_tokens".to_string(), Value::from(max_tokens));

    for key in ["temperature", "top_p", "metadata"] {
        if let Some(value) = body.get(key).filter(|value| !value.is_null()) {
            out.insert(key.to_string(), value.clone());
        }
    }
    if let Some(stops) = body.get("stop_sequences").filter(|value| is_truthy(value)) {
        out.insert("stop".to_string(), stops.clone());
    }
    // 工具：Anthropic 的 `{name, description, input_schema}` → Chat 的嵌套形态；
    // 服务端工具（`{"type":"web_search_20250305", …}`，上游自己执行）**保真携带**——
    // 它的 `type` 是准入凭据（DeepSeek 的 Anthropic 端点只认 web_search_*
    // 两个值，别的形态整轮 400），曾在这里被改写成一只**空壳 function**
    // （`name` 在、`type` 没了），症状正是「模型自带的搜索工具无法调用」
    // （issue #61）。去留交给出站侧按目标协议定，见 `native_tool` 模块头。
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let mut converted: Vec<Value> = Vec::new();
        let mut natives: Vec<Value> = Vec::new();
        for tool in tools {
            if native_tool::is_native_anthropic(tool) {
                natives.push(native_tool::carry(tool, native_tool::ORIGIN_ANTHROPIC));
                continue;
            }
            if let Some(function) = tool_to_chat(tool) {
                converted.push(function);
            }
        }
        if !natives.is_empty() {
            logging::verbose(
                "[Anthropic]",
                &format!(
                    "原生（服务端执行）工具声明 {} 条随行：{}",
                    natives.len(),
                    natives
                        .iter()
                        .map(tool_plan::tool_kind_label)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            converted.extend(natives);
        }
        if !converted.is_empty() {
            out.insert("tools".to_string(), Value::Array(converted));
        }
    }
    if let Some(choice) = body.get("tool_choice").filter(|value| is_truthy(value)) {
        out.insert("tool_choice".to_string(), tool_choice_to_chat(choice));
    }
    // 思考档位：Anthropic 的 thinking/output_config → Chat 的 reasoning_effort。
    // 注意 budget 与 max_tokens 的关系（见模块头），这里只映射档位，
    // max_tokens 的抬高在 `effort_to_chat` 里一并完成。
    if let Some(effort) = anthropic_effort(body) {
        out.insert("reasoning_effort".to_string(), Value::String(effort));
    }
    Ok(Value::Object(out))
}

/// system 字段的文本（可以是字符串，也可以是 `{type:"text"}` 块数组）
fn system_text(system: &Value) -> String {
    if let Some(text) = system.as_str() {
        return text.to_string();
    }
    let parts = content_parts(system);
    parts
        .iter()
        .map(|part| string_field(part, "text"))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 一个内容块上的 cache_control（没有 / 不是对象 → None）
fn block_cache_control(part: &Value) -> Option<Value> {
    part.get("cache_control").filter(|value| value.is_object()).cloned()
}

/// 块数组里**最后一个**带 cache_control 的断点（Anthropic 的惯例是断点打在
/// 要缓存的前缀末尾块上，一条消息/一组 system 块里取最后一个即整段语义）
fn last_cache_control(parts: &[Value]) -> Option<Value> {
    parts.iter().rev().find_map(block_cache_control)
}

/// 一条 Anthropic 消息 → 一到两条 Chat 消息（追加进 `messages`）。
///
/// 之所以可能拆成两条：Anthropic 把 tool_result 放在 user 消息的 content 块里，
/// 而 Chat 要求 tool 结果独立成 `role:"tool"` 消息。一条 user 消息里若既有
/// 文本又有 tool_result，就要拆成「tool 消息」+「user 消息」—— **顺序不能反**：
/// 下游两条协议都要求结果紧跟发起调用的 assistant（理由见本函数内「工具结果」一段）。
fn convert_message(messages: &mut Vec<Value>, message: &Value) -> Result<(), ConvertError> {
    let role = string_field(message, "role").to_lowercase();
    if role != "user" && role != "assistant" {
        // Anthropic 只有 user / assistant 两个角色；其它值原样当 user 处理
        // （报错会让一个只写错 role 的请求整个失败，而它其实能跑）
    }
    let content = message.get("content").unwrap_or(&Value::Null);
    // 字符串形态：直接一条消息
    if let Some(text) = content.as_str() {
        let role = if role == "assistant" { "assistant" } else { "user" };
        messages.push(json!({ "role": role, "content": text }));
        return Ok(());
    }
    let parts = content_parts(content);
    if parts.is_empty() {
        return Ok(());
    }

    // 分离三类块：工具结果（要独立成 tool 消息）、工具调用（进 tool_calls）、
    // 其余内容（文本/图片/思考，进常规消息）
    let mut tool_results: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut reasoning = String::new();
    let mut normal: Vec<Value> = Vec::new();
    // 块级 cache_control → 消息级暂存（粒度取舍见 mod.rs「内部暂存字段」的说明）。
    // 思考块按 Anthropic 的规则**不可**挂 cache_control，这里也不收。
    let mut normal_cache: Option<Value> = None;
    let mut tool_use_cache: Option<Value> = None;
    let mut tool_result_caches: Vec<Option<Value>> = Vec::new();
    for part in parts {
        if let Some(text) = part.as_str() {
            normal.push(json!({ "type": "text", "text": text }));
            continue;
        }
        let cache = block_cache_control(part);
        let kind = string_field(part, "type").to_lowercase();
        match kind.as_str() {
            "tool_result" => {
                tool_results.push(part.clone());
                tool_result_caches.push(cache.clone());
            }
            "tool_use" => {
                let id = string_field(part, "id");
                let arguments = {
                    let raw = json_text(part.get("input").unwrap_or(&Value::Null));
                    if raw.is_empty() { "{}".to_string() } else { raw }
                };
                tool_calls.push(json!({
                    "id": if id.is_empty() { random_id("call") } else { id },
                    "type": "function",
                    "function": {
                        "name": string_field(part, "name"),
                        "arguments": arguments,
                    },
                }));
                if cache.is_some() {
                    tool_use_cache = cache.clone();
                }
            }
            "thinking" | "redacted_thinking" => {
                let text = string_field(part, "thinking");
                if !text.is_empty() {
                    if !reasoning.is_empty() {
                        reasoning.push('\n');
                    }
                    reasoning.push_str(&text);
                }
            }
            "text" => normal.push(json!({ "type": "text", "text": string_field(part, "text") })),
            "image" => {
                if let Some(converted) = image_to_chat(part) {
                    normal.push(converted);
                }
            }
            _ => normal.push(part.clone()),
        }
        // 常规内容块的断点：最后一个带 cache_control 的块为准（顺序覆盖）
        if cache.is_some() && kind != "tool_result" && kind != "tool_use" {
            normal_cache = cache.clone();
        }
    }

    let chat_role = if role == "assistant" { "assistant" } else { "user" };

    // 工具结果：每条独立成 tool 消息，且必须排在**本消息正文之前**。
    //
    // Anthropic 允许同一条 user 消息里既带 tool_result 又带正文（Claude Desktop
    // 加载 skill 的注入形态），但下游两条协议都要求结果先落地：Chat 侧 tool 消息
    // 必须紧跟发起调用的 assistant（中间插任何消息都算配对断裂，上游判 11148
    // "tool calls and tool results do not match"），Anthropic 侧 tool_result 必须
    // 在 user 内容块最前（否则报 "Did not find 1 tool_result block(s) at the
    // beginning of this message"）。Anthropic 自身的规范顺序同样是 tool_result
    // 在前、正文在后 —— 先推正文会把用户消息插进 assistant 与其结果之间，
    // 严格上游因此对之后每条请求都 400，整条会话报废。
    //
    // 工具结果里的图片：Chat 的 tool 消息不许带图（OpenAI 直接 400，理由见
    // `responses::PendingImages`），所以攒起来、在本轮工具结果**全部**落地之后
    // 落成一条 user 消息。不需要跨 Anthropic 消息暂存 —— Anthropic 要求一条
    // assistant 的全部 tool_result 都在紧接着的那条 user 消息里。
    let mut tool_result_images: Vec<Value> = Vec::new();
    for (index, result) in tool_results.iter().enumerate() {
        let tool_use_id = string_field(result, "tool_use_id");
        let output = result.get("content").unwrap_or(&Value::Null);
        // is_error 是「这次工具执行失败了」的显式标记：丢掉后模型会把失败
        // 结果当正常输出继续推理。挂在内部暂存字段上，由 anthropic 出站
        // 恢复；OpenAI 形出口没有这个概念，随 strip 剥离。
        let is_error = result.get("is_error").and_then(Value::as_bool) == Some(true);
        let cache = tool_result_caches.get(index).cloned().flatten();
        let parts = tool_result_parts(output, image_to_chat);
        tool_result_images.extend(parts.images);
        let mut entry = Map::new();
        entry.insert("role".to_string(), Value::String("tool".to_string()));
        entry.insert(
            "tool_call_id".to_string(),
            Value::String(if tool_use_id.is_empty() { random_id("call") } else { tool_use_id }),
        );
        entry.insert(
            "content".to_string(),
            // 只有图片时 `text` 是空串，占位文案理由见 [`TOOL_IMAGE_PLACEHOLDER`]
            Value::String(if parts.text.is_empty() {
                TOOL_IMAGE_PLACEHOLDER.to_string()
            } else {
                parts.text
            }),
        );
        if is_error {
            entry.insert(FIELD_IS_ERROR.to_string(), Value::Bool(true));
        }
        if let Some(cache) = cache {
            entry.insert(FIELD_CACHE_CONTROL.to_string(), cache);
        }
        messages.push(Value::Object(entry));
    }
    // 结果之后、正文之前：图属于工具结果，挨着它最近的位置是这里。
    // （客户端自己的正文随后照旧成一条消息，两条连续 user 消息不做合并 ——
    // 上游接受这种形状，合并反而会把我们的合成内容融进客户端的消息）
    if !tool_result_images.is_empty() {
        messages.push(json!({ "role": "user", "content": Value::Array(tool_result_images) }));
    }

    // 工具调用：必须挂在 assistant 消息上（Anthropic 的 tool_use 只在 assistant 里）
    if !tool_calls.is_empty() {
        let mut entry = Map::new();
        entry.insert("role".to_string(), Value::String("assistant".to_string()));
        let text = content_text(&Value::Array(normal.clone()));
        entry.insert(
            "content".to_string(),
            if text.is_empty() { Value::Null } else { Value::String(text) },
        );
        if !reasoning.is_empty() {
            entry.insert("reasoning_content".to_string(), Value::String(reasoning.clone()));
        }
        entry.insert("tool_calls".to_string(), Value::Array(tool_calls));
        // 缓存断点优先取 tool_use 块上的（Claude Code 的惯例断点），否则用
        // 同一条消息里正文块的断点 —— 本来就是同一条 Anthropic 消息拆开的
        if let Some(cache) = tool_use_cache.or(normal_cache) {
            entry.insert(FIELD_CACHE_CONTROL.to_string(), cache);
        }
        messages.push(Value::Object(entry));
    } else if !normal.is_empty() || !reasoning.is_empty() {
        let mut entry = Map::new();
        entry.insert("role".to_string(), Value::String(chat_role.to_string()));
        entry.insert(
            "content".to_string(),
            collapse_content(Value::Array(normal.clone())),
        );
        // reasoning_content 只在 assistant 上有意义
        if chat_role == "assistant" && !reasoning.is_empty() {
            entry.insert("reasoning_content".to_string(), Value::String(reasoning));
        }
        if let Some(cache) = normal_cache {
            entry.insert(FIELD_CACHE_CONTROL.to_string(), cache);
        }
        messages.push(Value::Object(entry));
    }
    Ok(())
}

/// 工具结果拆出来的两部分（见 [`tool_result_parts`]）
pub(super) struct ToolResultParts {
    /// 文本部分（可能为空串 —— 只有图片时由调用方决定补什么）
    pub(super) text: String,
    /// 图片块，**已按调用方给的转换器转成目标协议的形态**
    pub(super) images: Vec<Value>,
}

/// 工具结果内容 → 文本 + 图片块。
///
/// `convert` 是该方向「图片块 → 目标协议形态」的转换器（入站给
/// [`image_to_chat`]，出站给 `image_to_anthropic`）：转不出来的块在这里就被
/// 剔除，所以调用方拿到的 `images` 一定落得下去，不必再判一次。
///
/// ── 为什么必须拆开（与 responses.rs 的 issue #76 同一类）──────
/// Anthropic 的 `tool_result.content` 允许**嵌套内容块数组**，官方收
/// `text` / `image` / `document` / `search_result` 四类，图片是真实用法
/// （Claude Code 读一张截图、MCP 的截图类工具都这么回）。原实现一律走
/// [`content_text`] 取文本，**图片被静默丢掉**；只有图片时则退化成 `json_text`
/// 把 base64 拍平成正文 —— 前者症状是模型看不到图却照上下文编答案（比 token
/// 暴涨更隐蔽，因为连 token 数都不涨），后者与 issue #76 完全同病。
///
/// 拆开之后：文本进 Chat 的 tool 消息，图片由调用方转交相邻的 user 消息
/// （Chat 的 tool 消息不许带图，理由见 `responses::PendingImages`）。
///
/// `pub(super)`：出站方向（`anthropic_outbound`）把 chat 的 tool 消息折回
/// `tool_result` 时用同一口径，两处各写一份迟早分叉。
pub(super) fn tool_result_parts(
    content: &Value,
    convert: fn(&Value) -> Option<Value>,
) -> ToolResultParts {
    let text_only = |text: String| ToolResultParts { text, images: Vec::new() };
    match content {
        Value::String(text) => {
            if text.is_empty() { text_only("(empty)".to_string()) } else { text_only(text.clone()) }
        }
        Value::Null => text_only("(empty)".to_string()),
        Value::Array(parts) => {
            let images: Vec<Value> = parts
                .iter()
                .filter(|part| is_image_block(part))
                .filter_map(|part| convert(part))
                .collect();
            let text = content_text(content);
            if text.is_empty() && images.is_empty() {
                // 一个块都落不下来（例如 Anthropic Files API 的 `file` 源图片、
                // 只有 `search_result` / `document` 的结果）：保持原来的拍平
                // 兜底，免得整段结果凭空消失
                let raw = json_text(content);
                return text_only(if raw.is_empty() { "(empty)".to_string() } else { raw });
            }
            ToolResultParts { text, images }
        }
        other => {
            let raw = json_text(other);
            text_only(if raw.is_empty() { "(empty)".to_string() } else { raw })
        }
    }
}

/// 是不是图片块（Anthropic 的 `image` 与 Chat 的 `image_url` / `input_image`
/// 都认 —— 两个方向的输入各是其中一种，转不转得出来交给 `convert` 判）
fn is_image_block(part: &Value) -> bool {
    matches!(
        string_field(part, "type").to_lowercase().as_str(),
        "image" | "image_url" | "input_image"
    )
}

/// Anthropic 的 image 块 → Chat 的 image_url 块。
///
/// Anthropic 的 base64 源（`{type:"base64", media_type, data}`）要拼成 data URI，
/// 因为 Chat 侧只认 URL 形态。
fn image_to_chat(part: &Value) -> Option<Value> {
    let source = part.get("source")?;
    let kind = string_field(source, "type").to_lowercase();
    match kind.as_str() {
        "base64" => {
            let media_type = string_field(source, "media_type");
            let data = string_field(source, "data");
            if data.is_empty() {
                return None;
            }
            let media = if media_type.is_empty() { "image/png".to_string() } else { media_type };
            Some(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{media};base64,{data}") },
            }))
        }
        "url" => {
            let url = string_field(source, "url");
            if url.is_empty() {
                None
            } else {
                Some(json!({ "type": "image_url", "image_url": { "url": url } }))
            }
        }
        _ => None,
    }
}

/// 内容块数组 → Chat content（只有纯文本时降级成字符串）
fn collapse_content(content: Value) -> Value {
    let parts = match content.as_array() {
        Some(parts) => parts.clone(),
        None => return content,
    };
    if parts.is_empty() {
        return Value::String(String::new());
    }
    let only_text = parts.iter().all(|part| {
        let kind = string_field(part, "type").to_lowercase();
        kind.is_empty() || kind == "text"
    });
    if only_text {
        let text: String = parts
            .iter()
            .map(|part| string_field(part, "text"))
            .collect();
        return Value::String(text);
    }
    Value::Array(parts)
}

/// Anthropic 工具 → Chat 工具
fn tool_to_chat(tool: &Value) -> Option<Value> {
    let name = string_field(tool, "name");
    if name.is_empty() {
        return None;
    }
    let parameters = tool
        .get("input_schema")
        .filter(|value| is_truthy(value))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let mut function = Map::new();
    function.insert("name".to_string(), Value::String(name));
    if let Some(description) = tool.get("description").filter(|value| is_truthy(value)) {
        function.insert("description".to_string(), description.clone());
    }
    function.insert("parameters".to_string(), parameters);
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String("function".to_string()));
    out.insert("function".to_string(), Value::Object(function));
    // 工具定义上的缓存断点（工具清单大而稳定，是 Anthropic 缓存收益最高的
    // 一段）随内部暂存字段携带，出站时恢复到 anthropic 工具对象上
    if let Some(cache) = block_cache_control(tool) {
        out.insert(FIELD_CACHE_CONTROL.to_string(), cache);
    }
    Some(Value::Object(out))
}

/// Anthropic 的 tool_choice → Chat 的 tool_choice
///
/// 两家的语义对应：`auto`→`auto`、`any`→`required`、`none`→`none`、
/// `{type:"tool", name}`→`{type:"function", function:{name}}`。
fn tool_choice_to_chat(choice: &Value) -> Value {
    if let Some(text) = choice.as_str() {
        return Value::String(text.to_string());
    }
    let kind = string_field(choice, "type").to_lowercase();
    match kind.as_str() {
        "any" => Value::String("required".to_string()),
        "auto" => Value::String("auto".to_string()),
        "none" => Value::String("none".to_string()),
        "tool" => {
            let name = string_field(choice, "name");
            if name.is_empty() {
                Value::String("auto".to_string())
            } else {
                json!({ "type": "function", "function": { "name": name } })
            }
        }
        _ => Value::String("auto".to_string()),
    }
}

/// Anthropic 的思考档位 → Chat 的 `reasoning_effort`
///
/// 取值链（参考实现同款）：`thinking.type` 决定开关，`output_config.effort`
/// 决定档位。`disabled`/`none` 一律不输出（让上游用它自己的默认）。
fn anthropic_effort(body: &Value) -> Option<String> {
    let thinking_type = {
        let raw = string_value(
            body.get("thinking").and_then(|value| value.get("type")).unwrap_or(&Value::Null),
        );
        raw.trim().to_lowercase()
    };
    if matches!(thinking_type.as_str(), "none" | "off" | "disabled") {
        return None;
    }
    let explicit = string_value(
        body.get("output_config").and_then(|value| value.get("effort")).unwrap_or(&Value::Null),
    );
    let explicit = explicit.trim().to_lowercase();
    if !explicit.is_empty() {
        // Anthropic 的 "max" 对应本项目的 "xhigh"（各家的最高档命名不一）
        return Some(if explicit == "max" { "xhigh".to_string() } else { explicit });
    }
    // thinking.enabled 但没给档位：按 budget 反推一个
    if thinking_type == "enabled" || thinking_type == "adaptive" {
        let budget = body
            .get("thinking")
            .and_then(|value| value.get("budget_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        return Some(effort_from_budget(budget).to_string());
    }
    None
}

/// budget → 档位
fn effort_from_budget(budget: i64) -> &'static str {
    if budget <= 0 {
        return "medium";
    }
    if budget <= 1024 {
        "low"
    } else if budget <= 4096 {
        "medium"
    } else if budget <= 10240 {
        "high"
    } else {
        "xhigh"
    }
}

// ─── 响应：Chat → Anthropic（非流式）────────────────────────

/// Chat 的 `chat.completion` → Anthropic 的 `message` 对象。
pub fn anthropic_from_chat(chat: &Value, model: &str) -> Value {
    let choice = chat.pointer("/choices/0");
    let message = choice.and_then(|choice| choice.get("message")).cloned().unwrap_or(Value::Null);
    let finish = choice
        .and_then(|choice| choice.get("finish_reason"))
        .map(string_value)
        .unwrap_or_default();

    let mut content: Vec<Value> = Vec::new();
    // 思考块在前（与 Anthropic 官方顺序一致）
    let reasoning = {
        let from_field = string_field(&message, "reasoning_content");
        if from_field.is_empty() { string_field(&message, "reasoning") } else { from_field }
    };
    if !reasoning.is_empty() {
        content.push(json!({ "type": "thinking", "thinking": reasoning }));
    }
    let text = content_text(message.get("content").unwrap_or(&Value::Null));
    if !text.is_empty() {
        content.push(json!({ "type": "text", "text": text }));
    }
    let mut has_tool = false;
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            has_tool = true;
            let id = string_field(call, "id");
            content.push(json!({
                "type": "tool_use",
                "id": if id.is_empty() { random_id("toolu") } else { id },
                "name": call.pointer("/function/name").map(string_value).unwrap_or_default(),
                "input": parse_json_object(
                    call.pointer("/function/arguments").unwrap_or(&Value::Null)
                ),
            }));
        }
    }
    // Anthropic 要求 content 非空
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "" }));
    }
    let stop_reason = if finish == "length" {
        "max_tokens"
    } else if finish == "tool_calls" || has_tool {
        "tool_use"
    } else if finish == "stop" || finish.is_empty() {
        "end_turn"
    } else {
        // 其它 finish_reason（content_filter 等）：Anthropic 没有对应值，
        // 归到 end_turn（比编一个不存在的值好）
        "end_turn"
    };
    let id = {
        let raw = string_field(chat, "id");
        if raw.is_empty() { random_id("msg") } else { raw }
    };
    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": usage_to_anthropic(chat.get("usage")),
    })
}

/// 把工具参数（字符串或对象）解析成对象
///
/// `pub(super)`：出站方向（`anthropic_outbound`）的 tool_use 块 input 也要
/// 「保证是对象」这同一归一（上游对非对象 input 会直接 400）。
pub(super) fn parse_json_object(value: &Value) -> Value {
    if value.is_object() {
        return value.clone();
    }
    let text = json_text(value);
    if text.is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(parsed) if parsed.is_object() => parsed,
        // 解析不了（上游给了非 JSON 的参数）：包一层，保证 input 是对象
        _ => json!({ "raw": text }),
    }
}

/// Chat 的 usage → Anthropic 的 usage
/// 输入 token 的粗估（CJK 一字一 token，其余约 4 字符一 token）。
///
/// 只用于 `message_start` 的占位：上游真实用量要到流末尾才出现，而
/// `message_start` 必须在首帧就发。真实值随后在 `message_delta` 覆盖。
pub fn estimate_chat_tokens(chat: &Value) -> i64 {
    let payload = json!({
        "messages": chat.get("messages").cloned().unwrap_or_else(|| json!([])),
        "tools": chat.get("tools").cloned().unwrap_or_else(|| json!([])),
    });
    let text = payload.to_string();
    let (mut cjk, mut total) = (0i64, 0i64);
    for ch in text.chars() {
        total += 1;
        if is_cjk_char(ch) {
            cjk += 1;
        }
    }
    cjk + ((total - cjk) + 3) / 4
}

fn is_cjk_char(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11ff
        | 0x2e80..=0x9fff
        | 0xac00..=0xd7af
        | 0xf900..=0xfaff
        | 0xff00..=0xffef)
}

pub fn usage_to_anthropic(usage: Option<&Value>) -> Value {
    let Some(usage) = usage.filter(|value| value.is_object()) else {
        return json!({ "input_tokens": 0, "output_tokens": 0 });
    };
    let number = |key: &str| -> i64 {
        usage.get(key).and_then(Value::as_i64).unwrap_or(0)
    };
    let cached = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let input = number("prompt_tokens");
    json!({
        // Anthropic 的 input_tokens **不含**缓存命中部分，所以这里减掉
        "input_tokens": (input - cached).max(0),
        "output_tokens": number("completion_tokens"),
        "cache_read_input_tokens": cached,
    })
}

// ─── 流式：Chat SSE → Anthropic SSE ─────────────────────────

/// Chat 的 SSE 字节流 → Anthropic 的 SSE 字节流（状态机）。
///
/// ── Anthropic 流的事件顺序（客户端按这个顺序解析）───────────
///   message_start
///   content_block_start / content_block_delta / content_block_stop（每块一轮）
///   message_delta（带 stop_reason 与最终 usage）
///   message_stop
///
/// Anthropic 的块是**严格串行**的：一个块 stop 之后才能 start 下一个。
/// 所以下面的状态机维护 `block_open`，切换块类型时先关旧的。
pub struct AnthropicStream {
    buffer: SseLineBuffer,
    message_id: String,
    model: String,
    started: bool,
    finished: bool,
    /// 当前打开的块：类型 + 索引 + 累积的文本/参数
    block_open: bool,
    block_index: i64,
    block_type: String,
    block_text: String,
    block_tool_id: String,
    block_tool_name: String,
    block_arguments: String,
    saw_arguments: bool,
    has_tool: bool,
    stop_reason: Option<String>,
    usage: Option<Value>,
    input_tokens: i64,
}

impl AnthropicStream {
    pub fn new(model: &str) -> Self {
        Self::with_input_tokens(model, 0)
    }

    /// `input_tokens` 用于 `message_start` 的占位（上游真实用量要到流末尾
    /// 才出现）；`message_delta` 会用真实值覆盖。
    pub fn with_input_tokens(model: &str, input_tokens: i64) -> Self {
        Self {
            buffer: SseLineBuffer::new(),
            message_id: random_id("msg"),
            model: model.to_string(),
            started: false,
            finished: false,
            block_open: false,
            block_index: -1,
            block_type: String::new(),
            block_text: String::new(),
            block_tool_id: String::new(),
            block_tool_name: String::new(),
            block_arguments: String::new(),
            saw_arguments: false,
            has_tool: false,
            stop_reason: None,
            usage: None,
            input_tokens,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<bytes::Bytes> {
        let mut out = Vec::new();
        for payload in self.buffer.push(chunk) {
            match payload {
                None => out.extend(self.finish()),
                Some(data) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&data) {
                        out.extend(self.consume(&value));
                    }
                }
            }
        }
        out
    }

    pub fn finish(&mut self) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        for payload in self.buffer.finish() {
            if let Some(data) = payload {
                if let Ok(value) = serde_json::from_str::<Value>(&data) {
                    out.extend(self.consume(&value));
                }
            }
        }
        if self.finished {
            return out;
        }
        self.finished = true;
        out.extend(self.emit_start());
        out.extend(self.close_block());
        let stop_reason = self
            .stop_reason
            .clone()
            .unwrap_or_else(|| if self.has_tool { "tool_use".to_string() } else { "end_turn".to_string() });
        // 真实 Anthropic 流把 `input_tokens` / `cache_read_input_tokens` 放在
        // 收尾的 `message_delta`（开头那帧只报 0 占位，见
        // `anthropic_outbound::ChatFromAnthropicStream::absorb_usage`）。
        // 只回传 output_tokens 会让客户端（含 Claude Code 的 context-hud）
        // 认为本次请求没有输入，上下文用量永远是 0。
        out.push(self.event(
            "message_delta",
            json!({
                "delta": { "stop_reason": stop_reason, "stop_sequence": Value::Null },
                "usage": usage_to_anthropic(self.usage.as_ref()),
            }),
        ));
        out.push(self.event("message_stop", json!({})));
        out
    }

    fn consume(&mut self, chunk: &Value) -> Vec<bytes::Bytes> {
        if self.finished {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(error) = chunk.get("error").filter(|value| is_truthy(value)) {
            self.finished = true;
            out.extend(self.emit_start());
            let message = {
                let text = string_field(error, "message");
                if text.is_empty() { string_value(error) } else { text }
            };
            let kind = {
                let text = string_field(error, "type");
                if text.is_empty() { "api_error".to_string() } else { text }
            };
            out.push(self.event("error", json!({ "error": { "type": kind, "message": message } })));
            return out;
        }
        if let Some(id) = chunk.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
            if !self.started {
                self.message_id = id.to_string();
            }
        }
        if let Some(usage) = chunk.get("usage").filter(|value| value.is_object()) {
            // 上游的 usage 只在流末尾出现；`input_tokens` 留到收尾的
            // `message_delta` 一起下发（见 `finish`）。
            self.input_tokens = usage.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0);
            self.usage = Some(usage.clone());
        }
        let choice = chunk.pointer("/choices/0");
        if let Some(finish) = choice
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            self.stop_reason = Some(if finish == "length" {
                "max_tokens".to_string()
            } else if finish == "tool_calls" {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            });
        }
        out.extend(self.emit_start());
        let Some(delta) = choice.and_then(|choice| choice.get("delta")) else {
            return out;
        };
        // 思考增量 → thinking 块
        let reasoning = {
            let from_field = string_field(delta, "reasoning_content");
            if from_field.is_empty() { string_field(delta, "reasoning") } else { from_field }
        };
        if !reasoning.is_empty() {
            out.extend(self.ensure_block("thinking"));
            self.block_text.push_str(&reasoning);
            out.push(self.event(
                "content_block_delta",
                json!({
                    "index": self.block_index,
                    "delta": { "type": "thinking_delta", "thinking": reasoning },
                }),
            ));
        }
        // 正文增量 → text 块
        if let Some(text) = delta.get("content").and_then(Value::as_str).filter(|text| !text.is_empty()) {
            out.extend(self.ensure_block("text"));
            self.block_text.push_str(text);
            out.push(self.event(
                "content_block_delta",
                json!({
                    "index": self.block_index,
                    "delta": { "type": "text_delta", "text": text },
                }),
            ));
        }
        // 工具调用 → tool_use 块
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                out.extend(self.consume_tool(call));
            }
        }
        out
    }

    fn consume_tool(&mut self, call: &Value) -> Vec<bytes::Bytes> {
        let mut out = Vec::new();
        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
        let id = call.get("id").and_then(Value::as_str).unwrap_or("");
        // 新的工具调用（带 id/name 且与当前块不同）：关掉旧块、开新块
        let is_new = !id.is_empty() && id != self.block_tool_id;
        if is_new || (self.block_type != "tool_use" && !name.is_empty()) {
            out.extend(self.close_block());
            self.has_tool = true;
            self.block_open = true;
            self.block_index += 1;
            self.block_type = "tool_use".to_string();
            self.block_tool_id = if id.is_empty() { random_id("toolu") } else { id.to_string() };
            self.block_tool_name = name.to_string();
            self.block_arguments.clear();
            self.saw_arguments = false;
            out.push(self.event(
                "content_block_start",
                json!({
                    "index": self.block_index,
                    "content_block": {
                        "type": "tool_use",
                        "id": self.block_tool_id,
                        "name": self.block_tool_name,
                        "input": {},
                    },
                }),
            ));
        } else if self.block_type != "tool_use" {
            // 只有参数分片、没见到 id/name：补开一个块（名字待定）
            out.extend(self.close_block());
            self.has_tool = true;
            self.block_open = true;
            self.block_index += 1;
            self.block_type = "tool_use".to_string();
            self.block_tool_id = random_id("toolu");
            self.block_tool_name = String::new();
            self.block_arguments.clear();
            self.saw_arguments = false;
            out.push(self.event(
                "content_block_start",
                json!({
                    "index": self.block_index,
                    "content_block": {
                        "type": "tool_use",
                        "id": self.block_tool_id,
                        "name": "",
                        "input": {},
                    },
                }),
            ));
        }
        // 名字晚到（先来参数分片、后来 name）：补一次
        if !name.is_empty() && self.block_tool_name.is_empty() {
            self.block_tool_name = name.to_string();
        }
        let arguments = call.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("");
        if !arguments.is_empty() {
            self.saw_arguments = true;
            self.block_arguments.push_str(arguments);
            out.push(self.event(
                "content_block_delta",
                json!({
                    "index": self.block_index,
                    "delta": { "type": "input_json_delta", "partial_json": arguments },
                }),
            ));
        }
        out
    }

    /// 确保当前打开的是指定类型的块（类型不同则先关旧的）
    fn ensure_block(&mut self, kind: &str) -> Vec<bytes::Bytes> {
        if self.block_open && self.block_type == kind {
            return Vec::new();
        }
        let mut out = self.close_block();
        self.block_open = true;
        self.block_index += 1;
        self.block_type = kind.to_string();
        self.block_text.clear();
        let content_block = if kind == "thinking" {
            json!({ "type": "thinking", "thinking": "" })
        } else {
            json!({ "type": "text", "text": "" })
        };
        out.push(self.event(
            "content_block_start",
            json!({ "index": self.block_index, "content_block": content_block }),
        ));
        out
    }

    fn close_block(&mut self) -> Vec<bytes::Bytes> {
        if !self.block_open {
            return Vec::new();
        }
        self.block_open = false;
        // 工具块收尾前若一个参数分片都没收到，补一个空对象：
        // Anthropic 的客户端在 input_json_delta 缺失时解析不出 input
        let mut out = Vec::new();
        if self.block_type == "tool_use" && !self.saw_arguments {
            out.push(self.event(
                "content_block_delta",
                json!({
                    "index": self.block_index,
                    "delta": { "type": "input_json_delta", "partial_json": "{}" },
                }),
            ));
        }
        out.push(self.event("content_block_stop", json!({ "index": self.block_index })));
        out
    }

    fn emit_start(&mut self) -> Vec<bytes::Bytes> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        vec![self.event(
            "message_start",
            json!({
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": { "input_tokens": self.input_tokens, "output_tokens": 0 },
                },
            }),
        )]
    }

    fn event(&self, event: &str, mut data: Value) -> bytes::Bytes {
        if let Some(map) = data.as_object_mut() {
            map.insert("type".to_string(), Value::String(event.to_string()));
        }
        event_frame(event, &data)
    }
}

/// 非流式聚合：把 Chat SSE 字节流收成一个 Anthropic message 对象。
pub struct AnthropicCollector {
    buffer: SseLineBuffer,
    text: String,
    reasoning: String,
    tools: std::collections::BTreeMap<i64, ToolAccum>,
    usage: Option<Value>,
    finish_reason: Option<String>,
    id: String,
}

#[derive(Default)]
struct ToolAccum {
    call_id: String,
    name: String,
    arguments: String,
}

impl AnthropicCollector {
    pub fn new() -> Self {
        Self {
            buffer: SseLineBuffer::new(),
            text: String::new(),
            reasoning: String::new(),
            tools: std::collections::BTreeMap::new(),
            usage: None,
            finish_reason: None,
            id: String::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        for payload in self.buffer.push(chunk) {
            if let Some(data) = payload {
                if let Ok(value) = serde_json::from_str::<Value>(&data) {
                    self.consume(&value);
                }
            }
        }
    }

    pub fn finish(&mut self) {
        for payload in self.buffer.finish() {
            if let Some(data) = payload {
                if let Ok(value) = serde_json::from_str::<Value>(&data) {
                    self.consume(&value);
                }
            }
        }
    }

    fn consume(&mut self, chunk: &Value) {
        if let Some(id) = chunk.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
            if self.id.is_empty() {
                self.id = id.to_string();
            }
        }
        if let Some(usage) = chunk.get("usage").filter(|value| value.is_object()) {
            self.usage = Some(usage.clone());
        }
        let choice = chunk.pointer("/choices/0");
        if let Some(finish) = choice
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            self.finish_reason = Some(finish.to_string());
        }
        let Some(delta) = choice.and_then(|choice| choice.get("delta")) else {
            return;
        };
        let reasoning = {
            let from_field = string_field(delta, "reasoning_content");
            if from_field.is_empty() { string_field(delta, "reasoning") } else { from_field }
        };
        self.reasoning.push_str(&reasoning);
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.text.push_str(text);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let key = call.get("index").and_then(Value::as_i64).unwrap_or(0);
                let entry = self.tools.entry(key).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                    entry.call_id = id.to_string();
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    if !name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                    entry.arguments.push_str(arguments);
                }
            }
        }
    }

    pub fn into_response(self, model: &str) -> Value {
        let mut content: Vec<Value> = Vec::new();
        if !self.reasoning.is_empty() {
            content.push(json!({ "type": "thinking", "thinking": self.reasoning }));
        }
        if !self.text.is_empty() {
            content.push(json!({ "type": "text", "text": self.text }));
        }
        let mut has_tool = false;
        for (_, tool) in self.tools {
            has_tool = true;
            let arguments = if tool.arguments.is_empty() { "{}".to_string() } else { tool.arguments };
            content.push(json!({
                "type": "tool_use",
                "id": if tool.call_id.is_empty() { random_id("toolu") } else { tool.call_id },
                "name": if tool.name.is_empty() { "unknown".to_string() } else { tool.name },
                "input": parse_json_object(&Value::String(arguments)),
            }));
        }
        if content.is_empty() {
            content.push(json!({ "type": "text", "text": "" }));
        }
        let stop_reason = match self.finish_reason.as_deref() {
            Some("length") => "max_tokens",
            Some("tool_calls") => "tool_use",
            _ if has_tool => "tool_use",
            _ => "end_turn",
        };
        let id = if self.id.is_empty() { random_id("msg") } else { self.id.clone() };
        json!({
            "id": id,
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": content,
            "stop_reason": stop_reason,
            "stop_sequence": Value::Null,
            "usage": usage_to_anthropic(self.usage.as_ref()),
        })
    }
}

impl Default for AnthropicCollector {
    fn default() -> Self {
        Self::new()
    }
}
/// 把网关错误映射成 Anthropic 的错误类型名（官方枚举）
pub fn error_kind_for_status(status: u16) -> &'static str {
    match status {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        500..=599 => "api_error",
        _ => "api_error",
    }
}

/// 把网关错误映射成 Responses 的错误码
pub fn responses_error_code(status: u16) -> &'static str {
    match status {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        500..=599 => "server_error",
        _ => "api_error",
    }
}
