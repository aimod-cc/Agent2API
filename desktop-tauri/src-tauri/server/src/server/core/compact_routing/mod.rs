//! 客户端压缩请求识别。只匹配当前请求末尾的完整模板，不搜索历史里的关键词。

use std::borrow::Cow;
use serde_json::Value;

pub(crate) mod parameters;

pub const ROUTE_FIELD: &str = "_wb_compact_route";

// 来源：ZCode core/compact/prompt.ts。模型用途仅存在于 CLI 内部，HTTP 没有专用端点或用途头。
const ZCODE_PREAMBLE: &str = "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n\n\
- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.\n\
- You already have all the context you need in the conversation above.\n\
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.\n\
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.\n\n";
const ZCODE_TASK: &str = "Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.";
const ZCODE_TRAILER: &str = "\n\nREMINDER: Do NOT call any tools. Respond with plain text only — an <analysis> block followed by a <summary> block. Tool calls will be rejected and you will fail the task.";
const ZCODE_STRUCTURE: &[&str] = &[
    "This summary should be thorough in capturing technical details, code patterns, and architectural decisions",
    "Before providing your final summary, wrap your analysis in <analysis> tags",
    "Your summary should include the following sections:",
    "1. Primary Request and Intent:",
    "6. All user messages:",
    "9. Optional Next Step:",
    "Here's an example of how your output should be structured:",
    "</summary>\n</example>",
    "Please provide your summary based on the conversation so far, following this structure",
];

fn template_matches(text: &str) -> bool {
    let normalized = if text.contains('\r') { Cow::Owned(text.replace("\r\n", "\n")) } else { Cow::Borrowed(text) };
    let Some(rest) = normalized.trim().strip_prefix(ZCODE_PREAMBLE) else { return false; };
    if !rest.starts_with(ZCODE_TASK) || !rest.ends_with(ZCODE_TRAILER) { return false; }
    // 固定段必须顺序出现；用户的 Additional Instructions 允许变化，不参与特征选择。
    let mut remaining = rest;
    for marker in ZCODE_STRUCTURE {
        let Some((_, tail)) = remaining.split_once(marker) else { return false; };
        remaining = tail;
    }
    true
}

#[derive(Clone, Copy)]
pub enum RequestFormat { Chat, Responses, Anthropic }

pub fn client_for_request(client: &str, body: &Value, format: RequestFormat) -> Option<&'static str> {
    if client != "ZCode" { return None; }
    let settings = crate::server::config::compact_routes::current();
    if settings.settings.as_ref().is_ok_and(|config| !config.enabled || config.rules.is_empty()) { return None; }
    let messages = body.get(match format { RequestFormat::Responses => "input", _ => "messages" })?.as_array()?;
    let last = messages.last()?;
    if last.get("role")?.as_str()? != "user" { return None; }
    // 前文至少有一次助手响应；孤立粘贴模板不是已发生的会话压缩。
    if !messages[..messages.len().saturating_sub(1)].iter().any(|message|
        message.get("role").and_then(Value::as_str) == Some("assistant")
            || matches!(format, RequestFormat::Responses) && message.get("type").and_then(Value::as_str) == Some("function_call")) { return None; }
    let content = last.get("content")?;
    let matches = if let Some(text) = content.as_str() {
        template_matches(text)
    } else {
        // 必须在协议翻译前读：Anthropic 的连续 user 会合并为内容块，翻成 Chat 后纯文本块会被拼成一个字符串。
        let part = content.as_array()?.last()?;
        let text_type = match format { RequestFormat::Responses => "input_text", _ => "text" };
        if part.get("type").and_then(Value::as_str) != Some(text_type) { return None; }
        part.get("text").and_then(Value::as_str).is_some_and(template_matches)
    };
    matches.then_some(crate::server::config::compact_routes::CLIENT_ZCODE)
}

pub fn mark_routed(body: &mut Value, source: &str, target: &str, client: &str) {
    if let Some(object) = body.as_object_mut() {
        object.insert(ROUTE_FIELD.into(), serde_json::json!({ "source": source, "target": target, "client": client }));
    }
}

pub fn take_route(body: &mut Value) -> Option<Value> {
    body.as_object_mut()?.remove(ROUTE_FIELD)
}
