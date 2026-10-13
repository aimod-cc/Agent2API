//! 上游 **Antigravity（Gemini v1internal）** 的出站转换（**请求侧**）：
//! chat 请求 → v1internal 信封请求。
//!
//! ── 本家三个文件的协作（单文件行数约定）──────────────────────
//! ```text
//!   antigravity_outbound.rs  本文件：请求信封转换 + tool_call 签名回填
//!   antigravity_schema.rs    工具参数 JSON Schema 清洗（坑 #13 的纯函数库）
//!   antigravity_stream.rs    响应侧状态机（Gemini SSE → chat SSE，
//!                            `ChatFromAntigravityStream`）
//! ```
//! 分派的第三个分支在 `upstream::provider_loop`（`UpstreamResponse::
//! AntigravityGemini`），壳在 `upstream::translate::AntigravityToChatStream`。
//! 三处一起构成与 Anthropic（ZCode 活动套餐）、NDJSON（Command Code）并列的
//! 第三条翻译通道：账号轮换、限额冷却、退避重试、usage 记账与取消处理
//! 全部留在编排层，只有字节形态在翻译器里变。
//!
//! ── 请求信封（规格 §3.3）───────────────────────────────────
//! ```jsonc
//! {
//!   "project": "<cloudaicompanionProject>",   // 缺失时不发该键（见下）
//!   "model": "gemini-pro-agent",              // **上游真名**，映射见 models.rs
//!   "userAgent": "antigravity",               // 企业账号（非 gmail）→ "jetski"
//!   "requestId": "agent/<ts_ms>/<8hex>",      // Manager 的格式（规格 §8.7 存疑）
//!   "requestType": "image_gen",               // **仅图片生成**；agent 路径不发（§8.1）
//!   "request": {
//!     "systemInstruction": { "role": "user", "parts": [ { "text": "…" } ] },
//!     "tools": [ { "functionDeclarations": [ … ] } ],   // **单分组**（坑 #13）
//!     "toolConfig": { "functionCallingConfig": { "mode": "VALIDATED" } },
//!     "generationConfig": { "temperature": 1.0, "topP": 1.0, "topK": 40,
//!                           "maxOutputTokens": 40960,
//!                           "thinkingConfig": { "includeThoughts": true,
//!                                               "thinkingBudget": 32768 } },
//!     "sessionId": "<稳定负整数>",
//!     "contents": [ { "role": "user" | "model", "parts": [ … ] } ]
//!   }
//! }
//! ```
//!
//! ── 请求侧逐条决策（都能在参考里找到出处）────────────────────
//!   1. **`contents` 必须以 user 结尾**（坑 #7）：末尾是 model 时补一条
//!      `{"role":"user","parts":[{"text":"Please continue your analysis."}]}`
//!      （Manager `TRANSIT_DEFENSE_FALLBACK_TEXT` 逐字）；首轮不是 user 时
//!      前面补一条 `"..."` 占位（9router `normalizeGeminiContents` 逐字）；
//!   2. **`functionResponse` 的 role 必须是 `user`**（坑 #9）：tool 消息统一
//!      折进 user 轮；连续同角色轮次合并（9router `normalizeGeminiContents`）；
//!   3. **工具名不改名**（本仓决定）：规格坑 #13 的函数名清洗针对的是**非法
//!      字符**，而改名的代价是 functionCall/functionResponse 配对错位 ——
//!      客户端声明的名字原样透传（9router 的 `sanitizeFunctionName` 改名与
//!      Manager 的 `_ide` 后缀都不复刻，报告里已注明）；
//!   4. **`tools` 只有一组 `functionDeclarations`**（坑 #13）、按名字去重；
//!      `tool_choice: "none"` 时不发 tools；`tool_choice` 的其它形态不翻译
//!      （两家参考对 Antigravity 都是无条件 `VALIDATED`）；
//!   5. **schema 清洗**（坑 #13）：见 `antigravity_schema.rs`（`type` 转小写、
//!      去掉上游不认的 JSON Schema 键、展开 `anyOf/oneOf/allOf`、空对象补
//!      `reason` 占位）；
//!   6. **`thinkingBudget`**：显式档位（客户端或映射绑定写进 body 的
//!      `reasoning_effort`）→ Manager 的档位规范值（1000/4000/10000）；
//!      否则目录条目的 `thinkingBudget`；再否则目录认它支持思考时给
//!      [`DEFAULT_THINKING_BUDGET`]。**`maxOutputTokens > thinkingBudget` 必须
//!      成立**（坑 #10）：不够就抬到 `budget + 8192`（Manager 同款），
//!      上限 [`MAX_OUTPUT_TOKENS`]（9router 的 64000，Manager 的逐模型上限
//!      本仓拿不到 —— 见报告）；
//!   7. **`requestType` 默认不发**（规格 §8.1 的已定决策：两参考矛盾，
//!      发了有「假 429」风险）；只有图片生成模型带 `"image_gen"`，
//!      并照 Manager 去掉 `tools`/`systemInstruction`（图片生成不支持）；
//!   8. **`sessionId` 稳定派生**：Manager `derive_session_id` 的 FNV-1a 变体
//!      逐字（官方客户端发「大负整数」），哈希输入取
//!      `project|model|system|首条 user 文本` —— 同一会话内稳定（上游
//!      server-side 缓存认它），不同会话不撞（规格 §8 未给出官方输入，
//!      这是本仓写明的派生法）；
//!   9. **`requestId` 每次唯一**：`agent/<ts_ms>/<8hex>`（Manager 格式；
//!      规格 §8.7 记录了两参考格式不一致、上游是否强校验未确认）；
//!  10. **系统提示词清洗**（坑 #12）：只落两套参考**逐字一致**的两条规则 ——
//!      Claude Agent SDK 前导整句删除、`opencode` 大小写保持改写成
//!      Antigravity。Manager 的 `x-*-billing*` 伪头剥离（带代码块保护）与
//!      9router 那条 recon 拷贝损坏的「Claude Code 前导句」规则**不做**，
//!      见报告（不猜不可读的字面量）。
//!
//! ── thoughtSignature 的往返方案与限制（响应侧见 `antigravity_stream.rs`）──
//! 两套参考都在服务端**按 call id / 会话缓存签名**，客户端历史不带时回填缓存值
//! 或默认签名（Manager 还会打 `skip_thought_signature_validator` 哨兵）。
//! OpenAI chat 的 wire 形态里**没有签名字段**，本仓又刻意不做跨请求状态缓存，
//! 因此采用「**响应侧随 tool_call 增量带出、请求侧原样回填、不带就省略**」：
//!   - 响应侧：`parts[*].thoughtSignature` 挂到对应 tool_call 的
//!     `extra_content.google.thought_signature`（Google 自家 OpenAI 兼容端点的
//!     官方字段位置，见 `antigravity_stream.rs`）；
//!   - 请求侧：本文件的 [`call_signature`] 从同一位置（也容忍
//!     `thought_signature` / `thoughtSignature` 裸字段）读回并回填到
//!     `functionCall` part；
//!   - 限制：客户端不回带签名时**省略**（不回填默认值 —— 那是参考实现的自有
//!     状态，本仓不发明）；非流式客户端走聚合器时 `extra_content` 会被
//!     `merge_tool_call` 丢掉（聚合形态只保留 id/type/name/arguments），
//!     签名只在流式路径可见。这些都是报告里点名的已知限制。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Map, Value};

use super::antigravity_schema::clean_schema;
use super::responses::ConvertError;
use super::{content_parts, content_text, is_truthy, string_field};
use crate::server::core::model_rules;
use crate::server::core::providers::antigravity::models;

// ─── 常量（出处逐条注明，别顺手改）────────────────────────────

/// `contents` 末尾是 model 时补的用户轮文本（Manager `TRANSIT_DEFENSE_FALLBACK_TEXT` 逐字）
const FALLBACK_USER_TEXT: &str = "Please continue your analysis.";
/// 首轮不是 user 时补的占位（9router `normalizeGeminiContents` 逐字）
const LEADING_USER_TEXT: &str = "...";
/// `maxOutputTokens` 上限（9router `MAX_ANTIGRAVITY_OUTPUT_TOKENS`）
const MAX_OUTPUT_TOKENS: i64 = 64_000;
/// `maxOutputTokens` 至少比 `thinkingBudget` 多这么多（Manager `budget + 8192`）
const THINKING_HEADROOM: i64 = 8_192;
/// 目录拿不到 `thinkingBudget` 时的兜底预算（规格 §3.3 官方客户端样例值）
const DEFAULT_THINKING_BUDGET: i64 = 32_768;
/// 图片生成的 `maxOutputTokens`（9router 图片分支逐字）
const IMAGE_MAX_OUTPUT_TOKENS: i64 = 8_192;
/// Claude Agent SDK 前导指纹声明（Manager/9router 都整句删除，坑 #12）
const CLAUDE_SDK_PREAMBLE: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

// ─── 请求：chat → v1internal 信封 ────────────────────────────

/// Chat Completions 请求体 → v1internal **完整信封**。
///
/// 分工：本函数负责**全部 wire 形态**（信封字段、`requestId`、`sessionId` 派生、
/// contents/tools/generationConfig 的转换）；调用方（适配器）只提供三样
/// 账号层事实 —— 上游真名 `model`（映射已在 `models.rs` 落地）、
/// `project`（账号记录里的 cloudaicompanionProject，空则不写信封里的 `project`
/// 键）、`enterprise`（企业账号用了 `jetski` 标记，见规格 §3.3）。
pub fn antigravity_request_from_chat(
    chat: &Value,
    model: &str,
    project: &str,
    enterprise: bool,
) -> Result<Value, ConvertError> {
    let messages = chat
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "缺少 messages 数组".to_string())?;
    let image_gen = is_image_model(model);
    let system = system_instruction(messages);
    // sessionId 的派生输入要先取出来（system 稍后被 move 进信封）
    let system_text = system
        .as_ref()
        .and_then(|value| value.pointer("/parts/0/text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let contents = contents_of(messages)?;

    let mut request = Map::new();
    if !image_gen {
        if let Some(system) = system {
            request.insert("systemInstruction".to_string(), system);
        }
        if let Some(declarations) = function_declarations(chat) {
            request.insert(
                "tools".to_string(),
                json!([{ "functionDeclarations": declarations }]),
            );
            // 两家参考对 Antigravity 都无条件 VALIDATED（规格 §3.3 的示例）
            request.insert(
                "toolConfig".to_string(),
                json!({ "functionCallingConfig": { "mode": "VALIDATED" } }),
            );
        }
    }
    request.insert(
        "generationConfig".to_string(),
        generation_config(chat, model, image_gen),
    );
    let first_user = first_user_text(&contents);
    request.insert(
        "sessionId".to_string(),
        Value::String(session_id(project, model, &system_text, &first_user)),
    );
    request.insert("contents".to_string(), Value::Array(contents));

    let mut envelope = Map::new();
    let project = project.trim();
    if !project.is_empty() {
        envelope.insert("project".to_string(), Value::String(project.to_string()));
    }
    envelope.insert("model".to_string(), Value::String(model.to_string()));
    envelope.insert(
        "userAgent".to_string(),
        Value::String(if enterprise { "jetski" } else { "antigravity" }.to_string()),
    );
    envelope.insert("requestId".to_string(), Value::String(request_id()));
    // §8.1 已定决策：agent 路径**不发** requestType（两参考矛盾，发了有假 429
    // 风险）；只有图片生成带自己的桶标记。
    if image_gen {
        envelope.insert(
            "requestType".to_string(),
            Value::String("image_gen".to_string()),
        );
    }
    envelope.insert("request".to_string(), Value::Object(request));
    Ok(Value::Object(envelope))
}

/// 消息角色（小写；缺失/空视为 user —— 与 9router `role || "user"` 同口径）
fn role_of(message: &Value) -> String {
    let raw = string_field(message, "role").trim().to_ascii_lowercase();
    if raw.is_empty() {
        "user".to_string()
    } else {
        raw
    }
}

/// 系统提示词 → `systemInstruction`（多条 system/developer 空行拼接 + 清洗）
fn system_instruction(messages: &[Value]) -> Option<Value> {
    let mut texts: Vec<String> = Vec::new();
    for message in messages {
        let role = role_of(message);
        if role != "system" && role != "developer" {
            continue;
        }
        let text = sanitize_prompt(&content_text(message.get("content").unwrap_or(&Value::Null)));
        if !text.trim().is_empty() {
            texts.push(text.trim().to_string());
        }
    }
    if texts.is_empty() {
        return None;
    }
    Some(json!({ "role": "user", "parts": [{ "text": texts.join("\n\n") }] }))
}

/// 系统提示词清洗（坑 #12：含竞品品牌会触发**假 429**）。
///
/// 只落两套参考**逐字一致**的规则：
///   1. Claude Agent SDK 前导整句删除（Manager `RE_WAF_TRIGGER_HEADERS` 与
///      9router `ANTIGRAVITY_PROMPT_REWRITES` 都有这一条，字面量一致）；
///   2. `opencode` → Antigravity（9router 的映射逐字：`OpenCode`→`Antigravity`、
///      `OPENCODE`→`ANTIGRAVITY`、其余大小写组合→`antigravity`）。
///
/// **不做的**（如实报告，不猜）：Manager 的 `x-*-billing*` / `cc_version` /
/// `cc_entrypoint` 伪头剥离（那是带代码块保护的独立实现）；9router 里那条
/// 「Claude Code 前导句」重写规则在 recon 拷贝里已损坏（正则字面量不可读）。
/// 只清洗 systemInstruction：客户端品牌落在系统提示词上，用户正文里的
/// 同名词不该被网关改写。
fn sanitize_prompt(text: &str) -> String {
    let stripped = text.replace(CLAUDE_SDK_PREAMBLE, "");
    rewrite_opencode(&stripped)
}

/// `/opencode/gi` 的 Rust 形态（大小写保持的整词替换，按字节扫描、只在
/// ASCII 匹配点切片 —— 不会切到 UTF-8 字符中间）。
fn rewrite_opencode(text: &str) -> String {
    const NEEDLE: &[u8] = b"opencode";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut index = 0usize;
    while index + NEEDLE.len() <= bytes.len() {
        if !bytes[index..index + NEEDLE.len()].eq_ignore_ascii_case(NEEDLE) {
            index += 1;
            continue;
        }
        out.push_str(&text[last..index]);
        out.push_str(match &text[index..index + NEEDLE.len()] {
            "OpenCode" => "Antigravity",
            "OPENCODE" => "ANTIGRAVITY",
            _ => "antigravity",
        });
        index += NEEDLE.len();
        last = index;
    }
    out.push_str(&text[last..]);
    out
}

/// messages → Gemini `contents`（见模块头 1/2/3 条）。
fn contents_of(messages: &[Value]) -> Result<Vec<Value>, ConvertError> {
    // tool_call_id → 工具名：functionResponse.name 必须与 functionCall 的一致
    let mut names: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for message in messages {
        if role_of(message) != "assistant" {
            continue;
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let id = string_field(call, "id").trim().to_string();
                let name = string_field(call.pointer("/function").unwrap_or(&Value::Null), "name")
                    .trim()
                    .to_string();
                if !id.is_empty() && !name.is_empty() {
                    names.insert(id, name);
                }
            }
        }
    }

    let mut contents: Vec<Value> = Vec::new();
    let mut dropped_reasoning = 0usize;
    for message in messages {
        match role_of(message).as_str() {
            "system" | "developer" => continue,
            "tool" => {
                let id = string_field(message, "tool_call_id").trim().to_string();
                let name = names
                    .get(&id)
                    .cloned()
                    .or_else(|| {
                        let name = string_field(message, "name").trim().to_string();
                        (!name.is_empty()).then_some(name)
                    })
                    .unwrap_or_else(|| "tool".to_string());
                let text = content_text(message.get("content").unwrap_or(&Value::Null));
                let mut response = Map::new();
                response.insert("name".to_string(), Value::String(name));
                response.insert(
                    "response".to_string(),
                    json!({ "result": tool_result_value(&text) }),
                );
                if !id.is_empty() {
                    response.insert("id".to_string(), Value::String(id));
                }
                // 坑 #9：functionResponse 必须挂在 user 轮
                push_turn(
                    &mut contents,
                    "user",
                    vec![json!({ "functionResponse": Value::Object(response) })],
                );
            }
            "assistant" => {
                let mut parts = content_parts_of(message.get("content").unwrap_or(&Value::Null), "assistant");
                if is_truthy(message.get("reasoning_content").unwrap_or(&Value::Null)) {
                    // 两参考的处理：9router 的请求侧直接丢掉纯思考 part；
                    // 没有签名可回填，带上去反而可能被上游拒（报告已注明限制）
                    dropped_reasoning += 1;
                }
                if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        if let Some(part) = function_call_part(call) {
                            parts.push(part);
                        }
                    }
                }
                if !parts.is_empty() {
                    push_turn(&mut contents, "model", parts);
                }
            }
            // user 与一切未识别的角色都当 user（9router 同口径）
            _ => {
                let parts = content_parts_of(message.get("content").unwrap_or(&Value::Null), "user");
                if !parts.is_empty() {
                    push_turn(&mut contents, "user", parts);
                }
            }
        }
    }
    if dropped_reasoning > 0 {
        crate::server::logging::verbose(
            "[Antigravity]",
            &format!(
                "历史里有 {dropped_reasoning} 条 assistant reasoning_content 未回填\
                 （OpenAI chat 形态没有可回带的思考签名，两参考的请求侧同样不带）"
            ),
        );
    }
    if contents.is_empty() {
        return Err("messages 里没有任何可发送的内容".to_string());
    }
    // 坑 #7：以 user 结尾；首轮必须是 user（Gemini 的硬性形态）
    if contents
        .last()
        .map(|turn| string_field(turn, "role") != "user")
        .unwrap_or(false)
    {
        contents.push(json!({ "role": "user", "parts": [{ "text": FALLBACK_USER_TEXT }] }));
    }
    if string_field(&contents[0], "role") != "user" {
        contents.insert(0, json!({ "role": "user", "parts": [{ "text": LEADING_USER_TEXT }] }));
    }
    Ok(contents)
}

/// 一条 assistant 的 chat `tool_call` → `functionCall` part（带签名回填）。
fn function_call_part(call: &Value) -> Option<Value> {
    let function = call.get("function").unwrap_or(&Value::Null);
    let name = string_field(function, "name").trim().to_string();
    if name.is_empty() {
        return None;
    }
    let mut body = Map::new();
    body.insert("name".to_string(), Value::String(name));
    body.insert(
        "args".to_string(),
        call_args(function.get("arguments").unwrap_or(&Value::Null)),
    );
    let id = string_field(call, "id").trim().to_string();
    if !id.is_empty() {
        body.insert("id".to_string(), Value::String(id));
    }
    let mut part = Map::new();
    part.insert("functionCall".to_string(), Value::Object(body));
    if let Some(signature) = call_signature(call) {
        part.insert("thoughtSignature".to_string(), Value::String(signature));
    }
    Some(Value::Object(part))
}

/// chat `function.arguments`（JSON 字符串 / 对象）→ Gemini `args` 对象。
/// Gemini 的 `args` 必须是结构体（对象），解析不出时给空对象（9router 同口径）。
fn call_args(value: &Value) -> Value {
    match value {
        Value::Object(_) => value.clone(),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        _ => json!({}),
    }
}

/// 历史里回带的思考签名（出站时挂到 tool_call 的
/// `extra_content.google.thought_signature` —— Google 自家 OpenAI 兼容端点的
/// 字段位置；也容忍裸 `thought_signature` / `thoughtSignature` 形态）。
fn call_signature(call: &Value) -> Option<String> {
    for pointer in [
        "/extra_content/google/thought_signature",
        "/thought_signature",
        "/thoughtSignature",
    ] {
        if let Some(signature) = call.pointer(pointer).and_then(Value::as_str) {
            let signature = signature.trim();
            if !signature.is_empty() {
                return Some(signature.to_string());
            }
        }
    }
    None
}

/// `tool` 消息的正文 → `functionResponse.response.result`。
///
/// 9router 的取值链逐字：正文先当 JSON 解析，解析出对象就用它，否则包一层
/// `{"result": 原文}`（Gemini 的 response 必须是对象）。
fn tool_result_value(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) => value,
        Ok(value @ Value::Array(_)) => value,
        Ok(scalar) if !scalar.is_null() => json!({ "result": scalar }),
        _ => json!({ "result": text }),
    }
}

/// 内容（字符串 / 块数组）→ parts：text 原样、`image_url` 的 data URI 内联。
///
/// http(s) 外链图片需要网关先下载才能内联，本步不做（记日志跳过 —— 猜一个
/// `fileData` 形态发出去只会换回 400，见报告）。
fn content_parts_of(content: &Value, role: &str) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            parts.push(json!({ "text": text }));
        }
        return parts;
    }
    for part in content_parts(content) {
        if let Some(text) = part.as_str() {
            if !text.is_empty() {
                parts.push(json!({ "text": text }));
            }
            continue;
        }
        let kind = string_field(part, "type").to_ascii_lowercase();
        match kind.as_str() {
            "text" | "input_text" | "output_text" => {
                let text = string_field(part, "text");
                if !text.is_empty() {
                    parts.push(json!({ "text": text }));
                }
            }
            "image_url" | "input_image" if role != "assistant" => match image_part(part) {
                Some(image) => parts.push(image),
                None => crate::server::logging::verbose(
                    "[Antigravity]",
                    "图片未内联：只认 data:image/...;base64 形态的 image_url，\
                     远程 URL 需要网关先下载（本步不做），已跳过该图片",
                ),
            },
            _ => {}
        }
    }
    parts
}

/// `image_url` → `inlineData{mimeType,data}`（规格 §4.3 的内联形态）。
fn image_part(part: &Value) -> Option<Value> {
    let source = part.get("image_url").unwrap_or(part);
    let url = match source {
        Value::String(text) => text.clone(),
        other => string_field(other, "url"),
    };
    let rest = url.trim().strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let media_type = meta.strip_suffix(";base64")?;
    if data.is_empty() {
        return None;
    }
    let mime_type = if media_type.is_empty() {
        "image/png"
    } else {
        media_type
    };
    Some(json!({ "inlineData": { "mimeType": mime_type, "data": data } }))
}

/// 连续同角色轮次合并（9router `normalizeGeminiContents` 同款）
fn push_turn(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if let Some(last) = contents.last_mut() {
        if string_field(last, "role") == role {
            if let Some(existing) = last.get_mut("parts").and_then(Value::as_array_mut) {
                existing.extend(parts);
                return;
            }
        }
    }
    contents.push(json!({ "role": role, "parts": parts }));
}

/// 首条 user 轮的文本（sessionId 派生用；没有 user 轮时给空串）
fn first_user_text(contents: &[Value]) -> String {
    contents
        .iter()
        .find(|turn| string_field(turn, "role") == "user")
        .and_then(|turn| turn.pointer("/parts/0/text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// chat `tools` → `functionDeclarations`（单分组、按名去重；名字不改）。
///
/// `tool_choice: "none"` 时返回 None（不发 tools）；`tool_choice` 的其它形态
/// （`required` / 点名）**不翻译** —— 两家参考对 Antigravity 都无条件
/// `VALIDATED`（见模块头第 4 条）。
fn function_declarations(chat: &Value) -> Option<Vec<Value>> {
    if tool_choice_is_none(chat) {
        return None;
    }
    let tools = chat.get("tools").and_then(Value::as_array)?;
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut declarations: Vec<Value> = Vec::new();
    for tool in tools {
        let function = tool.get("function").unwrap_or(tool);
        let name = string_field(function, "name").trim().to_string();
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        let mut declaration = Map::new();
        declaration.insert("name".to_string(), Value::String(name));
        if let Some(description) = function.get("description").filter(|value| is_truthy(value)) {
            declaration.insert("description".to_string(), description.clone());
        }
        declaration.insert(
            "parameters".to_string(),
            clean_schema(function.get("parameters").unwrap_or(&Value::Null)),
        );
        declarations.push(Value::Object(declaration));
    }
    if declarations.is_empty() {
        None
    } else {
        Some(declarations)
    }
}

/// `tool_choice` 是不是「不准调用工具」（字符串 `none` 或 `{type:"none"}`）
fn tool_choice_is_none(chat: &Value) -> bool {
    match chat.get("tool_choice") {
        Some(Value::String(text)) => text.trim().eq_ignore_ascii_case("none"),
        Some(Value::Object(object)) => object
            .get("type")
            .and_then(Value::as_str)
            .map(|kind| kind.trim().eq_ignore_ascii_case("none"))
            .unwrap_or(false),
        _ => false,
    }
}

// ─── generationConfig ───────────────────────────────────────

/// generationConfig：客户端参数 + Manager 的默认档 + thinkingConfig 与
/// maxOutputTokens 的不变量（坑 #10，见模块头第 6 条）。
fn generation_config(chat: &Value, model: &str, image_gen: bool) -> Value {
    if image_gen {
        // 9router 图片分支的固定档（图片生成不读客户端采样参数）
        return json!({
            "temperature": 1.0,
            "topP": 0.95,
            "topK": 40,
            "maxOutputTokens": IMAGE_MAX_OUTPUT_TOKENS,
        });
    }
    let mut config = Map::new();
    for (source, target) in [("temperature", "temperature"), ("top_p", "topP"), ("top_k", "topK")] {
        if let Some(value) = chat.get(source).filter(|value| value.is_number()) {
            config.insert(target.to_string(), value.clone());
        }
    }
    // 官方客户端默认档（Manager 在缺失时注入 topK=40 / topP=1.0）
    config
        .entry("topK".to_string())
        .or_insert_with(|| json!(40));
    config
        .entry("topP".to_string())
        .or_insert_with(|| json!(1.0));
    let client_max = ["max_tokens", "max_completion_tokens"]
        .iter()
        .find_map(|key| chat.get(*key).and_then(Value::as_i64).filter(|value| *value > 0));
    match thinking_budget(chat, model) {
        Some(budget) => {
            let mut max = client_max;
            if max.map(|value| value <= budget).unwrap_or(true) {
                max = Some(budget + THINKING_HEADROOM);
            }
            let mut max = max.map(|value| value.min(MAX_OUTPUT_TOKENS));
            // 不变量：maxOutputTokens > thinkingBudget（极端输入下压缩预算而不是违约）
            let budget = match max {
                Some(value) if value <= budget => value.saturating_sub(THINKING_HEADROOM).max(1),
                _ => budget.min(MAX_OUTPUT_TOKENS - THINKING_HEADROOM),
            };
            if let Some(value) = max.as_mut() {
                *value = (*value).max(budget.saturating_add(1)).min(MAX_OUTPUT_TOKENS);
            }
            config.insert(
                "thinkingConfig".to_string(),
                json!({ "includeThoughts": true, "thinkingBudget": budget }),
            );
            if let Some(value) = max {
                config.insert("maxOutputTokens".to_string(), Value::from(value));
            }
        }
        None => {
            if let Some(value) = client_max {
                config.insert(
                    "maxOutputTokens".to_string(),
                    Value::from(value.min(MAX_OUTPUT_TOKENS)),
                );
            }
        }
    }
    Value::Object(config)
}

/// 思考预算（见模块头第 6 条的取值链）；None = 不注入 thinkingConfig。
fn thinking_budget(chat: &Value, model: &str) -> Option<i64> {
    if let Some(level) = model_rules::read_client_level(chat) {
        if model_rules::reasoning_is_off(&level) {
            // 本仓的一贯取舍：没有安全的「关闭思考」表达（adapter 的
            // `reasoning_patch` 文档），不翻译成上游字段
            return None;
        }
        if let Some(budget) = level_budget(&level) {
            return Some(budget);
        }
    }
    if let Some(budget) = models::entry_number(model, "thinkingBudget").filter(|value| *value > 0) {
        return Some(budget);
    }
    if models::entry_bool(model, "supportsThinking").unwrap_or(false) {
        return Some(DEFAULT_THINKING_BUDGET);
    }
    None
}

/// 通用档位 → 预算（Manager 的客户端档位规范值：high/max/xhigh → 10000、
/// medium → 4000、low/extra-low/minimal → 1000；pro 的 10001 一档不复刻）
fn level_budget(level: &str) -> Option<i64> {
    Some(match model_rules::reasoning_rank(level)? {
        0 | 1 => 1000,
        2 => 4000,
        _ => 10000,
    })
}

// ─── sessionId / requestId ──────────────────────────────────

/// 稳定 `sessionId`：Manager `derive_session_id` 的 FNV-1a 变体逐字
/// （官方客户端发「大负整数」，两套参考都把它当上游会话键）。
/// 哈希输入见模块头第 8 条。
fn session_id(project: &str, model: &str, system: &str, first_user: &str) -> String {
    let mut hash: i64 = -3750763034362895579; // FNV offset basis（Manager 逐字）
    for byte in format!("{project}|{model}|{system}|{first_user}").bytes() {
        hash = hash.wrapping_mul(1099511628211);
        hash ^= i64::from(byte);
    }
    hash.to_string()
}

/// 每次唯一的 `requestId`：`agent/<ts_ms>/<8hex>`（Manager 格式，规格 §8.7）
fn request_id() -> String {
    let mut bytes = [0u8; 4];
    let suffix = match getrandom::getrandom(&mut bytes) {
        Ok(()) => bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        // 随机源失败（几乎不可能）时用时间戳低 32 位兜底：requestId 的唯一性
        // 只影响上游幂等判定，退化成时间戳也好过 panic
        Err(_) => format!("{:08x}", crate::server::logging::now_ms() as u64 as u32),
    };
    format!("agent/{}/{}", crate::server::logging::now_ms(), suffix)
}

/// 图片生成模型（按 id 判，规格 §5.2 的 `gemini-3-pro-image` /
/// `gemini-3.1-flash-image` 一族）
fn is_image_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("image")
}

