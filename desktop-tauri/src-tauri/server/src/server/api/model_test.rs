//! 模型测试 API（模型管理页操作列那颗「测试」）：
//!
//!   POST /api/models/test   `{provider, model, account_id?, prompt?, system_prompt?,
//!                            reasoning?, stream?, protocol?, proxy?, test_id?}`
//!                            → 一次真实请求的结果
//!
//! ── 为什么单独开一个模块 ─────────────────────────────────────
//! 它与 `model_manage.rs`（纯读写模型规则，不碰转发）和 `chat.rs`（对外网关面）
//! 都不同：它挂在管理面（要管理员会话 / 网关 Key）、却要**真打上游**，而且
//! 走的就是生产那条转发链路。放在一起会让「这一条到底算不算对外契约」得翻
//! 代码才知道；独立文件 + 独立登记（与 `models.rs` / `auto_checkin.rs` 同一分工）
//! 让分组在路由表上直接可见。
//!
//! ── 关键取舍：测试走**真实转发链路**，不自己拼一条 ─────────────
//! `UpstreamService::forward` 是唯一出口 —— token 刷新、429 换号与限额记账、
//! 出站指纹脱敏、系统提示词模式、协议翻译、思考等级注入、调试报文与请求日志
//! 全都在那条链路上。自己实现一条「内部转发」看着更简单，代价是结论与生产
//! 表现**迟早分叉**（参考项目 OmniProxy 在 `modelTestRun.ts` 里也走过这一步：
//! 它后来改成「按被测目标的原生协议打本机真实网关」）。
//!
//! ── 与生产请求的不同（靠 `ForwardRequest` 的收窄字段实现）──────
//!   1. **钉住这一家**：`allowed_providers = KeyScope::provider_only(provider)`
//!      —— 同一个对外名允许在多家各挂一条映射，不定家的话「测这一行」测出来的
//!      是别人，结论无法归因；
//!   2. **钉住这一个账号**：`pinned_account`（见 `core::upstream::rotate`），
//!      而且**不顺延** —— 问的是「这个账号行不行」，换个人跑通只会把结论搅浑；
//!   3. **不去重**：`dedupe_key` 传空串。去重是「相同 body 的快速重试排队」，
//!      而测试常常就要并发地对多个账号各发一次同样的 body（前端一个账号一条
//!      请求），去重会让它们互相等待、甚至只跑一条。
//!   4. **跳过模型的启停门禁**：`ignore_model_gate`。这颗按钮的用法是「先测通、
//!      再决定要不要启用」，被测的行往往就是关着的；生产链路上「承载家全被
//!      关闭」的模型以 404 拒之门外，那道门会把被测对象挡在门外。候选直接取
//!      第 1 条钉住的那家，发送侧按家改写照常走生产语义（关闭的绑定解析不出
//!      目标 → 名字原样直发、映射上的思考等级不注入）—— 测的正是「这个模型
//!      在这家上游的真实形态」，不是「开了开关之后大概会怎样」。
//!   5. **支持按次临时切换出网节点**：`proxy_override`。缺省跟随账号自身绑定的
//!      代理配置，也可在测试弹窗里临时指定「直连」或代理池中的任意节点，仅对本次
//!      测试请求生效、不改动账号落盘的代理设置。
//!   6. **支持网关对话协议三选一**：`protocol`（`chat_completions` / `responses` /
//!      `anthropic`）。按所选协议组装下游请求、经过 `core::protocol` 转换成内部
//!      Chat 形态送入转发层，收到上游回复后再按所选协议做回程转换与字段提取，
//!      一并验证协议转换链路是否畅通。
//!
//! 除这几处外，其它一切都与真实请求一致 —— 包括**会消耗额度**。
//!
//! ── 记账：进请求日志，但被报表排除 ────────────────────────────
//! 测试请求走的是同一条链路，所以它会像真实请求一样被记下来（`is_test = 1`）。
//! 请求日志照常显示（能对照、能翻查），报表聚合把它排除（人工反复发起的样本
//! 混进趋势图会把真实流量读歪）。进行中行也一样：先 `record_started`，
//! 收尾 `record_entry` 补全终态。
//!
//! ── 客户端断线 ──────────────────────────────────────────────
//! 与 `/v1/*` 同一套：`cancellation::register` 让这条测试可以被「终止请求」
//! 终止，`DisconnectGuard` 在 handler 被 axum 取消时补一条 408 终态
//! （否则那条进行中行会一直挂在请求日志里）。前端「中止测试」= 关掉这一层
//! 弹窗 = abort 掉 fetch，落到的就是这个守卫。
//!
//! ── 状态码口径：结果失败也是 2xx ─────────────────────────────
//! 「这次测试跑完了，结论是失败」与「这条管理请求本身没法处理」是两件事：
//! 上游错误、账号不可用、所选节点不可用这些结论都放在响应体的 `status` / `error` 里，
//! HTTP 一律 200（与 `/api/models/refresh` 永远 2xx 同一取舍），前端因此能拿到
//! 结构化的结论，而不是先去解析错误响应。真正非 2xx 的只有「请求本身不合法」
//! （缺 provider / model，或这个模型不在该家的清单里）。

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::key_scope::KeyScope;
use crate::server::core::protocol::{anthropic, responses};
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::catalog;
use crate::server::core::upstream::aggregate::aggregate_frame_stream;
use crate::server::core::upstream::cancellation;
use crate::server::core::upstream::sse::FramePolicy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::{ForwardOutcome, ForwardRequest};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

use super::disconnect_guard::DisconnectGuard;
use super::pipeline;

/// 用户提示词留空时的默认值（前端展示同一份文案）。
///
/// 取「你好」而不是一句有信息量的话：测试要的是**最小请求** —— 越短越省额度、
/// 越少触发上游的内容策略，也越容易看出「通不通」这件事本身。
pub const DEFAULT_TEST_PROMPT: &str = "你好";
/// 提示词长度上限（用户提示词与系统提示词各一）。
///
/// 与 OmniProxy 的 `MAX_TEST_PROMPT_LENGTH` 同值：测试不是聊天，一段超长提示词
/// 既没有意义（结论还是「通不通」），又可能把上游的限额 / 风控额度一次烧掉。
const MAX_PROMPT_CHARS: usize = 4000;
/// 单次测试的**总预算**（含等上游首帧与聚合完整段回答）。
///
/// 为什么测试要有自己的上界（而生产请求没有）：生产请求的等待由客户端自己决定
/// （它随时能断开），而测试是**弹窗里的一次等待** —— 没有上界时，一个上游卡住
/// 的账号会让弹窗一直转圈，用户既不知道该等多久、也不知道能不能关。
///
/// ── 为什么是 50 秒（这个数字被桌面壳的请求超时卡住）────────────
/// 桌面端的管理 API 调用经 `workbuddyDesktop` 桥走本机 HTTP，而那条通道有
/// **60 秒**总超时（`src/gateway.rs` 的 `REQUEST_TIMEOUT_MS`）。测试若跑得比它久，
/// 界面看到的是「连接本地代理超时」—— 一个与真实原因无关的错。
/// 所以这里的上界必须留在它之内：50 秒 ≤ 60 秒，客户端先拿到一个说明白的
/// 「测试超时」，而不是一个误导性的传输层错误。
const TEST_TIMEOUT: Duration = Duration::from_secs(50);

/// 网关对话协议（三选一，与 `/v1/chat/completions`、`/v1/responses`、`/v1/messages` 一一对应）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestProtocol {
    ChatCompletions,
    Responses,
    Anthropic,
}

impl TestProtocol {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "chat" | "chat_completions" => Ok(Self::ChatCompletions),
            "responses" => Ok(Self::Responses),
            "anthropic" | "messages" => Ok(Self::Anthropic),
            other => Err(format!(
                "不支持的对话协议 {other}（可选 chat_completions / responses / anthropic）"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
        }
    }
}

/// 从请求体里取一段可选文本（去空白、截断到上限；非字符串按空处理）。
fn text_field(body: &Value, key: &str, limit: usize) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .map(|text| pipeline::truncate_chars(text, limit))
        .unwrap_or_default()
}

/// 组装标准 Chat Completions 下游请求体。
///
/// ── 思考等级怎么带（「跟随映射」与「本次覆盖」在这里合流）─────────
/// 不填 `reasoning` 时**一个等级字段都不写进 body**：映射上绑定的等级由转发层
/// 按映射注入（`model_rules::reasoning` 的既有语义）；填了则写进
/// `reasoning_effort` —— 那是「客户端自己指定的档位」，按既有口径**优先于映射**
/// （映射不覆盖客户端显式指定的等级）。于是界面上那几个选项各自对应一条明确的
/// 路径，网关侧一行特判都不需要。
fn build_chat_body(
    model: &str,
    prompt: &str,
    system_prompt: &str,
    reasoning: &str,
    stream: bool,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !system_prompt.is_empty() {
        messages.push(json!({ "role": "system", "content": system_prompt }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));
    let mut body = json!({
        "model": model,
        "messages": messages,
        // 下游的意愿（`stream` 为 false 时转发层仍会以 stream:true 打上游、
        // 再聚合回 JSON —— 与真实客户端要非流式时走的是同一条路）
        "stream": stream,
    });
    if !reasoning.is_empty() {
        if let Some(object) = body.as_object_mut() {
            object.insert(
                "reasoning_effort".to_string(),
                Value::String(reasoning.to_string()),
            );
        }
    }
    body
}

/// 组装 OpenAI Responses 下游请求体（对应 `POST /v1/responses`）。
fn build_responses_body(
    model: &str,
    prompt: &str,
    system_prompt: &str,
    reasoning: &str,
    stream: bool,
) -> Value {
    let mut body = json!({
        "model": model,
        "input": prompt,
        "stream": stream,
    });
    if let Some(object) = body.as_object_mut() {
        if !system_prompt.is_empty() {
            object.insert(
                "instructions".to_string(),
                Value::String(system_prompt.to_string()),
            );
        }
        if !reasoning.is_empty() {
            object.insert("reasoning".to_string(), json!({ "effort": reasoning }));
        }
    }
    body
}

/// 组装 Anthropic Messages 下游请求体（对应 `POST /v1/messages`）。
fn build_anthropic_body(
    model: &str,
    prompt: &str,
    system_prompt: &str,
    reasoning: &str,
    stream: bool,
) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": 8192,
        "messages": [
            { "role": "user", "content": prompt }
        ],
        "stream": stream,
    });
    if let Some(object) = body.as_object_mut() {
        if !system_prompt.is_empty() {
            object.insert(
                "system".to_string(),
                Value::String(system_prompt.to_string()),
            );
        }
        if !reasoning.is_empty() {
            object.insert("thinking".to_string(), json!({ "type": "enabled" }));
            object.insert("output_config".to_string(), json!({ "effort": reasoning }));
        }
    }
    body
}

/// 按所选协议构造「下游原始请求体」与「入站翻译后的 Chat 转发请求体」。
fn build_protocol_request(
    protocol: TestProtocol,
    model: &str,
    prompt: &str,
    system_prompt: &str,
    reasoning: &str,
    stream: bool,
) -> Result<(Value, Value), String> {
    match protocol {
        TestProtocol::ChatCompletions => {
            let chat = build_chat_body(model, prompt, system_prompt, reasoning, stream);
            Ok((chat.clone(), chat))
        }
        TestProtocol::Responses => {
            let req = build_responses_body(model, prompt, system_prompt, reasoning, stream);
            let chat = responses::chat_from_responses(&req)?;
            Ok((req, chat))
        }
        TestProtocol::Anthropic => {
            let req = build_anthropic_body(model, prompt, system_prompt, reasoning, stream);
            let mut chat = anthropic::chat_from_anthropic(&req)?;
            // 测试弹窗里用户显式选定的思考档位（如 CatPaw 的 max / minimal）直接取自
            // 该模型的声明档位，保持原值写进 reasoning_effort，避免被通用映射改名
            if !reasoning.is_empty() {
                if let Some(object) = chat.as_object_mut() {
                    object.insert(
                        "reasoning_effort".to_string(),
                        Value::String(reasoning.to_string()),
                    );
                }
            }
            Ok((req, chat))
        }
    }
}

/// 从 chat 响应体里取一段文本（`choices[0].message.<key>`；没有则空串）。
fn message_field(completion: &Value, key: &str) -> String {
    completion
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 从 Responses 响应体里提取回复正文与思考摘要。
fn extract_responses_fields(response: &Value) -> (String, String) {
    let mut reply = response
        .get("output_text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut reasoning_parts: Vec<String> = Vec::new();
    let mut message_parts: Vec<String> = Vec::new();
    if let Some(output) = response.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str).unwrap_or("") {
                "reasoning" => {
                    if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                        for part in summary {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    reasoning_parts.push(text.to_string());
                                }
                            }
                        }
                    }
                }
                "message" => {
                    if let Some(content) = item.get("content").and_then(Value::as_array) {
                        for part in content {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    message_parts.push(text.to_string());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if reply.is_empty() && !message_parts.is_empty() {
        reply = message_parts.join("");
    }
    (reply, reasoning_parts.join(""))
}

/// 从 Anthropic Messages 响应体里提取回复正文与思考过程。
fn extract_anthropic_fields(response: &Value) -> (String, String) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut thinking_parts: Vec<String> = Vec::new();
    if let Some(content) = response.get("content").and_then(Value::as_array) {
        for block in content {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            text_parts.push(text.to_string());
                        }
                    }
                }
                "thinking" => {
                    if let Some(thinking) = block.get("thinking").and_then(Value::as_str) {
                        if !thinking.is_empty() {
                            thinking_parts.push(thinking.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (text_parts.join(""), thinking_parts.join(""))
}

/// 把聚合后的 Chat Completion 按所选下游协议翻译并提取结果。
fn translate_completion(
    protocol: TestProtocol,
    model: &str,
    protocol_request: &Value,
    status: i64,
    chat_completion: Value,
) -> TestOutcome {
    match protocol {
        TestProtocol::ChatCompletions => TestOutcome {
            status,
            error: None,
            reply: message_field(&chat_completion, "content"),
            reasoning_text: message_field(&chat_completion, "reasoning_content"),
            raw_response: pipeline::capture_raw_json(&chat_completion),
        },
        TestProtocol::Responses => {
            let response_body =
                responses::responses_from_chat(&chat_completion, model, protocol_request);
            let raw_response = pipeline::capture_raw_json(&response_body);
            if response_body.get("status").and_then(Value::as_str) == Some("failed") {
                let message = response_body
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .unwrap_or("Responses 协议转换返回 failed 状态")
                    .to_string();
                return TestOutcome {
                    status: 502,
                    error: Some(message),
                    reply: String::new(),
                    reasoning_text: String::new(),
                    raw_response,
                };
            }
            let (reply, reasoning_text) = extract_responses_fields(&response_body);
            TestOutcome {
                status,
                error: None,
                reply,
                reasoning_text,
                raw_response,
            }
        }
        TestProtocol::Anthropic => {
            let response_body = anthropic::anthropic_from_chat(&chat_completion, model);
            let raw_response = pipeline::capture_raw_json(&response_body);
            let (reply, reasoning_text) = extract_anthropic_fields(&response_body);
            TestOutcome {
                status,
                error: None,
                reply,
                reasoning_text,
                raw_response,
            }
        }
    }
}

/// 解析本次测试的临时节点覆盖（`proxy` 字段）：
///   - `""` / `"follow"`：跟随账号自身配置（`None`）
///   - `"direct"` / `"none"`：本次强制直连（`Some(None)`）
///   - `"pool:<id>"` 或 `"<id>"`：解析代理池条目（`Some(Some(proxy))`）
fn parse_proxy_override(
    raw: &str,
) -> Result<(Option<Option<ResolvedProxy>>, Option<String>), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("follow") {
        return Ok((None, None));
    }
    if trimmed.eq_ignore_ascii_case("direct") || trimmed.eq_ignore_ascii_case("none") {
        return Ok((Some(None), Some("直连".to_string())));
    }
    let proxy_id = trimmed.strip_prefix("pool:").unwrap_or(trimmed).trim();
    if proxy_id.is_empty() {
        return Err("缺少代理池节点 id".to_string());
    }
    let resolved = crate::server::core::proxy_pool::resolve_for_test(proxy_id)?;
    let label = if resolved.label.is_empty() {
        format!(
            "{} {}:{}",
            resolved.protocol.to_uppercase(),
            resolved.host,
            resolved.port.unwrap_or(0)
        )
    } else {
        resolved.label.clone()
    };
    Ok((Some(Some(resolved)), Some(label)))
}

/// 这一次测试跑出来的结论（三路结果收敛成同一个形态）。
struct TestOutcome {
    status: i64,
    error: Option<String>,
    reply: String,
    reasoning_text: String,
    raw_response: Option<String>,
}

/// POST /api/models/test
pub async fn run_model_test(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return errors::management_error(400, error.message),
    };
    let provider = text_field(&payload, "provider", 64);
    let model = text_field(&payload, "model", 128);
    if provider.is_empty() {
        return errors::management_error(400, "缺少 provider");
    }
    if model.is_empty() {
        return errors::management_error(400, "缺少 model");
    }
    // 模型必须**这一家认识**：清单里没有它时，转发层的收窄会以「不在这把网关
    // Key 的可用提供商列表里」收场 —— 那句话对测试场景毫无信息量，这里先拒掉
    // 并说清下一步（多半是清单没刷新，或者模型名被上游改名了）
    let carriers = catalog::providers_for_model(&model);
    if !carriers.iter().any(|id| id.eq_ignore_ascii_case(&provider)) {
        return errors::management_error(
            400,
            format!(
                "{provider} 的当前清单里没有模型 {model}：先点「获取模型」刷新清单，或确认模型名没被上游改名"
            ),
        );
    }
    let protocol_raw = text_field(&payload, "protocol", 32);
    let protocol = match TestProtocol::parse(&protocol_raw) {
        Ok(protocol) => protocol,
        Err(message) => return errors::management_error(400, message),
    };
    let account_id = text_field(&payload, "account_id", 128);
    let proxy_raw = text_field(&payload, "proxy", 128);
    let (proxy_override, proxy_label) = match parse_proxy_override(&proxy_raw) {
        Ok(parsed) => parsed,
        Err(reason) => {
            // 选中的代理池节点不可用（如 Clash 监听器未启动、节点已被删除）时，
            // 按测试失败结论（HTTP 200 + status=400）返回，前端结果层直接展示原因
            return ok_json(json!({
                "success": false,
                "status": 400,
                "error": reason,
                "reply": "",
                "reasoning": "",
                "provider": provider,
                "model": model,
                "protocol": protocol.as_str(),
                "proxy": Value::Null,
                "account_id": if account_id.is_empty() { Value::Null } else { Value::String(account_id) },
                "account_name": Value::Null,
                "upstream_model": Value::Null,
                "upstream_reasoning": Value::Null,
                "duration_ms": 0,
                "ttfb_ms": Value::Null,
                "attempts": 1,
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0,
            }));
        }
    };
    let prompt = {
        let text = text_field(&payload, "prompt", MAX_PROMPT_CHARS);
        if text.is_empty() {
            DEFAULT_TEST_PROMPT.to_string()
        } else {
            text
        }
    };
    let system_prompt = text_field(&payload, "system_prompt", MAX_PROMPT_CHARS);
    let reasoning = text_field(&payload, "reasoning", 32);
    // 缺省按流式（与前端默认一致）：流式才看得到首字延迟，而那是这个测试
    // 最有价值的一个读数
    let stream = payload.get("stream").and_then(Value::as_bool).unwrap_or(true);

    let (protocol_request, request_body) = match build_protocol_request(
        protocol,
        &model,
        &prompt,
        &system_prompt,
        &reasoning,
        stream,
    ) {
        Ok(pair) => pair,
        Err(message) => {
            return ok_json(json!({
                "success": false,
                "status": 400,
                "error": message,
                "reply": "",
                "reasoning": "",
                "provider": provider,
                "model": model,
                "protocol": protocol.as_str(),
                "proxy": proxy_label,
                "account_id": if account_id.is_empty() { Value::Null } else { Value::String(account_id) },
                "account_name": Value::Null,
                "upstream_model": Value::Null,
                "upstream_reasoning": Value::Null,
                "duration_ms": 0,
                "ttfb_ms": Value::Null,
                "attempts": 1,
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0,
            }));
        }
    };

    let started_at = logging::now_ms();
    // ── 关联 id 由**前端**给（`test_id`），拿不到才自己生成 ─────────
    // 为什么：前端要在测试跑着的时候就能把它掐掉，而掐的手法是既有的
    // 「终止请求」（`POST /api/stats/requests/terminate?id=`，按 id 置位取消
    // 令牌）—— 那条接口要一个 id，而桌面壳的桥（`invoke('api_request')`）
    // **没有 abort**：前端无法取消一次已经在跑的 invoke。于是把 id 的生成
    // 权交给前端：它先造一个 id、连同请求一起发上来，测试期间就能用同一个
    // id 去终止（请求日志里那一行的 id 也是它，三处对齐）。
    //
    // `ensure_id` 只在 id 为空时才写入，所以有前端 id 时要用**没带 id 的**
    // 槽位（`new`），否则会保留自己生成的那个
    let given_id = text_field(&payload, "test_id", 64);
    let telemetry = Arc::new(RequestTelemetry::new());
    if !given_id.is_empty() {
        telemetry.ensure_id(&given_id);
    }
    let test_id = telemetry.id();
    // 「进行中」行：与真实请求同一处时点（解析成功、转发开始前）。
    // `is_test = true` 一路带下去 —— 报表排除它、请求日志标记它。
    // 客户端给空串：测试是网关自己发起的，没有下游 UA —— 「谁发起」
    // 那件事已经由 `is_test` 表达（见 `request_stats::RequestEntry::client`）
    state
        .request_stats()
        .record_started(&test_id, started_at, &model, &model, &reasoning, "", true);
    if let Some(token) = cancellation::register(&test_id) {
        telemetry.set_cancel_token(token);
    }
    // 断线兜底：handler 被取消（前端关掉结果弹窗 = abort 掉 fetch）时补 408 终态。
    // 正常路径必须显式 `complete()`，见下面两条出口
    let mut guard = DisconnectGuard::new(state.request_stats(), test_id.clone());
    // 请求日志的下游请求原文记录按所选协议组装出的请求体（预览对话可直接还原气泡）；
    // 与真实流量同一道闸：仅调试模式开启时采集（见 `capture_raw_json`）
    let raw_request = pipeline::capture_raw_json(&protocol_request);

    let outcome = run_forward(
        &state,
        &telemetry,
        &provider,
        &account_id,
        protocol,
        &model,
        &protocol_request,
        request_body,
        stream,
        proxy_override,
    )
    .await;
    let finished_at = logging::now_ms();
    let snapshot = telemetry.snapshot();
    let ttfb_ms = snapshot
        .first_response_at
        .map(|at| (at - started_at).max(0));

    // 记账（进请求日志，报表侧按 is_test 排除）：与 /v1/* 用的是同一个
    // `record_entry`，于是测试的明细与真实请求逐字段同形
    let context = pipeline::RecordContext {
        stats: state.request_stats(),
        telemetry: telemetry.clone(),
        started_at,
        model: model.clone(),
        client_model: model.clone(),
        client_reasoning: reasoning.clone(),
        status: outcome.status,
        raw_request,
        raw_response: outcome.raw_response.clone(),
        is_test: true,
        // 客户端空串：测试没有下游 UA（与上面的 record_started 同口径）
        client: String::new(),
    };
    pipeline::record_entry(&context, None);
    guard.complete();

    ok_json(json!({
        "success": outcome.error.is_none(),
        "status": outcome.status,
        "error": outcome.error,
        "reply": outcome.reply,
        "reasoning": outcome.reasoning_text,
        "provider": provider,
        "model": model,
        "protocol": protocol.as_str(),
        "proxy": proxy_label,
        // 实际承载的家与账号（选路结果，取自 telemetry 快照）：正常情况就是
        // 请求里点名的那两个，但「账号被删 / 被禁用」这类情况下转发层给出的
        // 结论与请求不同，如实回报比回显请求参数有用
        "account_id": snapshot.account_id,
        "account_name": snapshot.account_name,
        "upstream_model": snapshot.upstream_model,
        "upstream_reasoning": snapshot.upstream_reasoning,
        "duration_ms": finished_at - started_at,
        "ttfb_ms": ttfb_ms,
        "attempts": snapshot.attempts.max(1),
        "prompt_tokens": snapshot.prompt_tokens,
        "completion_tokens": snapshot.completion_tokens,
        "total_tokens": snapshot.total_tokens,
    }))
}

/// 真正发一次：转发 + 把三路结果按所选协议收敛成 [`TestOutcome`]。
///
/// 流式与不流式的差别只在这里：转发层两种都接受，但**流式那一支必须由调用方
/// 把流读完**（否则 `ForwardOutcome::Stream` 一被丢掉，这条请求就没结果了）。
/// 读它用的是网关非流式模式下同一个聚合器（`aggregate_frame_stream`），
/// 聚合后再按所选协议（Chat Completions / Responses / Anthropic Messages）
/// 做回程转换与字段提取。
#[allow(clippy::too_many_arguments)]
async fn run_forward(
    state: &ServerState,
    telemetry: &Arc<RequestTelemetry>,
    provider: &str,
    account_id: &str,
    protocol: TestProtocol,
    model: &str,
    protocol_request: &Value,
    request_body: Value,
    stream: bool,
    proxy_override: Option<Option<ResolvedProxy>>,
) -> TestOutcome {
    let forward = state.upstream().forward(ForwardRequest {
        body: request_body,
        stream,
        // 不去重（见模块头）：并发测多个账号时，去重会让它们互相等待
        dedupe_key: String::new(),
        // 空头：测试不是某个客户端发来的，转发层也不该把面板的管理头透传给
        // 上游（适配器会读会话类头，见 `passthrough_session_headers`）
        client_headers: Default::default(),
        telemetry: telemetry.clone(),
        // 钉住这一家（同一个对外名可能挂在多家上，见模块头）
        allowed_providers: Some(KeyScope::provider_only(provider)),
        // 钉住这一个账号，且不顺延
        pinned_account: if account_id.is_empty() {
            None
        } else {
            Some(account_id.to_string())
        },
        // 未启用的行也要能测 —— 「先测通、再决定要不要启用」正是这颗按钮的用法。
        // 候选直接取上面钉住的那一家，生产的「模型已在网关中关闭」门禁不适用于
        // 测试（取舍见 `ForwardRequest::ignore_model_gate`）。
        ignore_model_gate: true,
        // 本次测试临时指定的出网代理 / 直连（None = 跟随账号自身配置）
        proxy_override,
    });
    // 总预算套在整段转发之外（含流式聚合）：测试是一次弹窗等待，必须有上界，
    // 否则一个卡住的上游会让弹窗一直转圈（见 TEST_TIMEOUT）
    match tokio::time::timeout(TEST_TIMEOUT, forward).await {
        Err(_elapsed) => TestOutcome {
            status: 504,
            error: Some(format!("测试超时（{} 秒内没拿到完整回答）", TEST_TIMEOUT.as_secs())),
            reply: String::new(),
            reasoning_text: String::new(),
            raw_response: None,
        },
        Ok(Err(error)) => TestOutcome {
            status: i64::from(error.status_code),
            error: Some(error.message),
            reply: String::new(),
            reasoning_text: String::new(),
            raw_response: None,
        },
        Ok(Ok(ForwardOutcome::Completion { body })) => {
            translate_completion(protocol, model, protocol_request, 200, body)
        }
        Ok(Ok(ForwardOutcome::Stream { status, stream: source })) => {
            let status = i64::from(status);
            // `ForwardOutcome::Stream` 给的是 `Box<dyn Stream + Unpin>`，聚合器要
            // 的是 `BoxStream`（`Pin<Box<dyn Stream>>`）—— `boxed()` 这一步就是
            // 把这个 Unpin 的盒子换成 `'static` 的 Pin 盒子，元素类型不变
            use futures::StreamExt;
            match tokio::time::timeout(
                TEST_TIMEOUT,
                aggregate_frame_stream(source.boxed(), telemetry.clone(), FramePolicy::default()),
            )
            .await
            {
                Err(_elapsed) => TestOutcome {
                    status: 504,
                    error: Some(format!(
                        "测试超时（{} 秒内没拿到完整回答）",
                        TEST_TIMEOUT.as_secs()
                    )),
                    reply: String::new(),
                    reasoning_text: String::new(),
                    raw_response: None,
                },
                Ok(Err(error)) => TestOutcome {
                    status: i64::from(error.status_code),
                    error: Some(error.message),
                    reply: String::new(),
                    reasoning_text: String::new(),
                    raw_response: None,
                },
                Ok(Ok(aggregated)) => {
                    translate_completion(protocol, model, protocol_request, status, aggregated.body)
                }
            }
        }
    }
}