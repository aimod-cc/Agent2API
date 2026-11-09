//! 客户端标题生成请求识别。
//!
//! ── 判据从哪来 ────────────────────────────────────────────────
//! 标题请求不是独立端点，也没有统一的用途头，各家的标记方式并不一样：
//!
//!   · **Codex** 用请求头显式标注。它把「不是对话轮次」的调用写在
//!     `x-openai-subagent`（`thread_title` / `thread_title_reconsideration` /
//!     `title_generation`），或者写在 `x-codex-turn-metadata` 的
//!     `thread_source` 里；Responses 的 `client_metadata` 里还有一份同样的
//!     `x-codex-turn-metadata`（Codex 的规范传输是 body，头部只是投影）。
//!     这套判据照搬参考实现 magpie 的 `session_parent.go` / `codex_titles.go`。
//!   · **ZCode** 没有用途头，但 system 提示词固定
//!     （`Generate a concise title for this coding session.`，见其
//!     `title-generation-sidecar`），且只发两条消息、不带工具。
//!   · **Claude Code** 的 user 消息以 `Please write a 5-10 word title` 开头。
//!
//! ── 为什么 Codex 不看提示词 ───────────────────────────────────
//! Codex 的标题提示词随版本变，且它的请求体里带着完整 schema；请求头是
//! 稳定的契约（Codex 自己就是这么告诉上游「这不是对话轮次」的），比猜措辞可靠。
//!
//! ── 为什么要求「不带工具」──
//! 提示词类判据（ZCode / Claude Code）只能靠措辞。带上工具说明的请求哪怕
//! 措辞相同也是正常对话（用户可能真的在问「请给这段对话写个标题」），
//! 把那种请求改写到便宜模型会答坏。Codex 有显式标记，不受这条约束。

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::config::title_routes::{self, CLIENT_CLAUDE_CODE, CLIENT_CODEX, CLIENT_ZCODE};

/// 改写痕迹字段：随请求体带到转发层，由 `take_route` 取出后写进请求记录。
pub const ROUTE_FIELD: &str = "_wb_title_route";

/// ZCode 标题 sidecar 的 system 提示词首句。取首句而不是全文：
/// 后半段是给模型的措辞约束，客户端改版时更容易动。
const ZCODE_TITLE_PREAMBLE: &str = "Generate a concise title for this coding session.";
/// Claude Code 的标题请求开头（`5-10` 与 `5–10` 两种连字符都出现过）。
const CLAUDE_TITLE_PREFIXES: &[&str] = &["Please write a 5-10 word title", "Please write a 5–10 word title"];

/// Codex 在 `x-openai-subagent` / `thread_source` 里用来标记标题请求的取值。
/// 前两个是线程标题（首次生成 / 重新考虑），第三个是旧版写法。
const CODEX_TITLE_KINDS: &[&str] = &["thread_title", "thread_title_reconsideration", "title_generation"];

/// 归一化：统一换行、去掉首尾空白，便于做前缀匹配。
fn normalized(text: &str) -> String {
    if text.contains('\r') { text.replace("\r\n", "\n") } else { text.to_string() }
}

fn text_of_content(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() { return Some(text.to_string()); }
    let parts = content.as_array()?;
    let mut joined = String::new();
    for part in parts {
        // Anthropic 的 text 块与 Responses 的 input_text 块同形。
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if !joined.is_empty() { joined.push('\n'); }
            joined.push_str(text);
        }
    }
    if joined.is_empty() { None } else { Some(joined) }
}

fn role_text(messages: &[Value], role: &str) -> Option<String> {
    let mut joined = String::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some(role) { continue; }
        if let Some(text) = message.get("content").and_then(text_of_content) {
            if !joined.is_empty() { joined.push('\n'); }
            joined.push_str(&text);
        }
    }
    if joined.is_empty() { None } else { Some(joined) }
}

/// 请求是否携带工具（含工具结果）。带工具的一律不当作提示词类标题请求。
fn has_tools(body: &Value) -> bool {
    if body.get("tools").and_then(Value::as_array).is_some_and(|tools| !tools.is_empty()) { return true; }
    if body.get("tool_choice").is_some() { return true; }
    body.get("messages").and_then(Value::as_array).is_some_and(|messages| messages.iter().any(|message| {
        matches!(message.get("role").and_then(Value::as_str), Some("tool") | Some("function"))
            || message.get("tool_calls").and_then(Value::as_array).is_some_and(|calls| !calls.is_empty())
    }))
}

/// Responses 协议的 `input` 数组里也可能带 function_call 项。
fn responses_has_tools(body: &Value) -> bool {
    has_tools(body) || body.get("input").and_then(Value::as_array).is_some_and(|items| {
        items.iter().any(|item| matches!(item.get("type").and_then(Value::as_str),
            Some("function_call") | Some("function_call_output") | Some("custom_tool_call")))
    })
}

/* ─────────────────── Codex：显式 kind ─────────────────── */

/// 从 `x-codex-turn-metadata` 里读 `thread_source`（Codex 自己的 JSON 结构）。
///
/// `user` / `subagent` / 空都表示「这是对话或子代理的正常轮次」，不算标题请求。
fn thread_source(metadata: &str) -> Option<String> {
    if !metadata.contains("thread_source") { return None; }
    let parsed: Value = serde_json::from_str(metadata).ok()?;
    let source = parsed.get("thread_source").and_then(Value::as_str).map(str::trim)?;
    match source {
        "" | "user" | "subagent" => None,
        other => Some(other.to_string()),
    }
}

fn is_codex_title_kind(kind: &str) -> bool {
    CODEX_TITLE_KINDS.iter().any(|value| *value == kind)
}

/// Codex 是否明确标注了这次请求是标题生成。
///
/// 三个来源依次看：`x-openai-subagent`、body 里 `client_metadata` 的
/// `x-codex-turn-metadata`、以及同名请求头。前两者是 Codex 的规范写法，
/// 头部那份是投影 —— 少看任何一个都会在某个版本上漏判。
fn codex_title(headers: &HeaderMap, body: &Value) -> bool {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
    // ① 直接声明「这不是对话轮次，是 subagent 调用」。
    if let Some(kind) = header("x-openai-subagent").filter(|value| !value.is_empty()) {
        return is_codex_title_kind(kind);
    }
    // ② body 里的 client_metadata（Codex 的规范传输）。
    if let Some(metadata) = body.pointer("/client_metadata/x-codex-turn-metadata") {
        // 这个成员既可能是 JSON 字符串，也可能已经是对象。
        let text = match metadata.as_str() {
            Some(text) => text.to_string(),
            None => metadata.to_string(),
        };
        if let Some(source) = thread_source(&text) { return is_codex_title_kind(&source); }
    }
    // ③ 头部投影的 turn metadata。
    if let Some(text) = header("x-codex-turn-metadata") {
        if let Some(source) = thread_source(text) { return is_codex_title_kind(&source); }
    }
    // ④ 没有元数据时，退回 client_metadata 里平铺的 thread_source（旧版客户端）。
    if let Some(source) = body.pointer("/client_metadata/thread_source").and_then(Value::as_str).map(str::trim) {
        if !matches!(source, "" | "user" | "subagent") { return is_codex_title_kind(source); }
    }
    false
}

/* ─────────────────── ZCode / Claude Code：提示词 ─────────────────── */

fn zcode_title(messages: &[Value]) -> bool {
    let Some(system) = role_text(messages, "system") else { return false; };
    if !normalized(&system).trim_start().starts_with(ZCODE_TITLE_PREAMBLE) { return false; }
    // 标题请求固定两条消息（system + 一段素材）；更多轮次说明这是正常对话。
    if messages.len() > 2 { return false; }
    role_text(messages, "user").is_some()
}

fn claude_title(messages: &[Value]) -> bool {
    let Some(user) = role_text(messages, "user") else { return false; };
    let text = normalized(&user);
    let trimmed = text.trim_start();
    CLAUDE_TITLE_PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix))
}

/// 判定这次请求属于哪家客户端的标题生成。
///
/// `client` 是行为客户端标识（`pipeline::request_source` 的 `behavior_client`），
/// 与压缩路由同一取值来源；只有配置里勾了这家客户端才继续判。
/// `headers` 给 Codex 的显式标记用；提示词类判据不看它。
pub fn client_for_request(
    client: &str,
    headers: &HeaderMap,
    body: &Value,
    responses: bool,
) -> Option<&'static str> {
    let settings = title_routes::active()?;
    // 这家客户端得有一条真会命中的规则（自己开着、勾了它、配了模型）才值得往下判 ——
    // 提示词判据要遍历消息结构，不便宜。
    if title_routes::match_rule(&settings, client).is_none() { return None; }

    // Codex 的判据是请求头，不需要看消息结构，先判它（省一次遍历）。
    if client == CLIENT_CODEX {
        return if codex_title(headers, body) { Some(CLIENT_CODEX) } else { None };
    }

    let messages = if responses {
        body.get("input").and_then(Value::as_array)
    } else {
        body.get("messages").and_then(Value::as_array)
    }?;
    if responses {
        if responses_has_tools(body) { return None; }
    } else if has_tools(body) {
        return None;
    }
    let matched = match client {
        CLIENT_ZCODE => zcode_title(messages),
        CLIENT_CLAUDE_CODE => claude_title(messages),
        _ => false,
    };
    if !matched { return None; }
    // 返回静态 id：调用方会把它带进改写痕迹与日志，不能是配置里的临时字符串。
    title_routes::CLIENTS.iter().find(|(id, _)| *id == client).map(|(id, _)| *id)
}

pub fn mark_routed(body: &mut Value, source: &str, target: &str, client: &str) {
    if let Some(object) = body.as_object_mut() {
        object.insert(ROUTE_FIELD.into(), serde_json::json!({ "source": source, "target": target, "client": client }));
    }
}

pub fn take_route(body: &mut Value) -> Option<Value> {
    body.as_object_mut()?.remove(ROUTE_FIELD)
}
