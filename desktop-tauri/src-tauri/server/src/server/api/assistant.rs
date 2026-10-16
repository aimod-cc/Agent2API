//! AI 助手 API（侧边栏「AI 助手」页的对话入口 + 设置页「AI 助手」分类的读写）。
//!
//!   GET  /api/assistant/config   → `{ config: {...} }`（含默认值展开，见下）
//!   PUT  /api/assistant/config   `{ config: {...} }` → 校验后整块保存
//!   POST /api/assistant/chat     `{ model?: {provider,id}, messages, tools? }`
//!                                → **SSE 流**（OpenAI 格式 delta，含 tool_calls）
//!
//! ── 为什么对话不在前端直接打 /v1/chat/completions ─────────────
//! 桌面壳的页面来自 Tauri 自定义协议（`tauri://localhost`），前端 fetch 到
//! `http://127.0.0.1:{port}/v1/*` 是**跨源**请求，而网关面默认不应答 CORS
//! （见 `KEY_CORS_ENABLED` 的说明）—— 那条路要求用户先打开 CORS 开关，
//! 等于让「用助手」依赖一个与它无关的安全开关。挂到面板路由（`/api/*`，
//! 无条件 CORS）就不必动那个开关，且对话框本来就是管理面的一部分。
//!
//! ── 与 /v1/chat/completions 的关系 ─────────────────────────
//! **共用同一条转发链路**（`UpstreamService::forward`）：token 刷新、429 换号、
//! 出站脱敏、协议翻译、调试报文都在那条链路上。本文件只做三件事：
//!   ① 把「用户选的模型」解析成上游名 —— 模型是对外名（`/v1/models` 的
//!      `data[].id`，映射名也算），走公共管道的 `resolve_model`（默认模型回落 /
//!      映射别名 / 目录校验与 /v1/* 同源）；**不钉提供商**，同名在多家时按账号
//!      优先级主备 —— 助手的路由语义与普通客户端完全一致；
//!   ② 按设置拼系统提示词（开关 × 工具是否为空，口径见 `AssistantConfig::system_prompt`）；
//!   ③ 流式回传（OpenAI 格式原样，前端自己解析 delta / tool_calls）。
//!
//! ── 工具定义与执行都在前端 ───────────────────────────────────
//! 本接口不认工具语义：`tools` 是前端按勾选过滤好的 OpenAI tools 数组，
//! 原样进上游请求；上游回的 `tool_calls` 原样流回前端，由前端执行、再把
//! `role: "tool"` 的结果作为下一轮的 messages 发上来。后端因此不认识任何一个
//! 工具 —— 加工具、改工具参数都不需要动 Rust 侧（工具的实现在前端，
//! 调的是已有的 /api/* 管理接口）。
//!
//! ── 记账 ────────────────────────────────────────────────────
//! 与模型测试同一口径：进请求日志（用户在「请求日志」里看得到助手的每一次
//! 消耗），`is_test = true` 使报表聚合排除它（人工反复发起的样本混进趋势图
//! 会把真实流量读歪）。`client` 留空：助手是网关自己发起的，没有下游 UA。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};

use crate::server::config::{self, AssistantConfig, MAX_SYSTEM_PROMPT_CHARS};
use crate::server::core::upstream::cancellation;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::{ForwardOutcome, ForwardRequest};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

use super::disconnect_guard::DisconnectGuard;
use super::pipeline::{self, json_response, RecordContext, RecordingStream, sse_response};

/// 对外模型名的长度上限（与 `model_test` 的 `text_field` 上限同一量级）
const MAX_MODEL_CHARS: usize = 128;
/// 单次对话里 messages 的条数上限。
///
/// 不是防用户，是防「前端 bug 把上下文堆到几千条」把网关的内存与上游的
/// 上下文窗口一起打爆：200 条足够一场长会话（每条都是完整一问一答）。
const MAX_MESSAGES: usize = 200;
/// tools 数组的长度上限（前端目前最多 9 只，留足余量）
const MAX_TOOLS: usize = 64;

/* ─── 设置（设置页「AI 助手」分类）───────────────────────── */

/// GET /api/assistant/config
///
/// 出站用 `status_value`：工具表展开成完整 9 项（缺省值在后端兜底，界面不必
/// 也维护一份默认集，见 `AssistantConfig::status_value`）。
pub async fn get_config(State(_state): State<ServerState>) -> Response {
    ok_json(json!({ "config": config::assistant_config().status_value() }))
}

/// PUT /api/assistant/config
///
/// 整块替换：前端拿 GET 的响应改一改变量再发回来，所以缺字段按默认值处理
/// （**不是**「缺字段 = 保留原值」—— 前端永远发全量，两条约定混用只会让
/// 「我明明删了这一项」变成解不开的谜）。
pub async fn put_config(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(block) = payload.get("config").and_then(Value::as_object) else {
        return errors::management_error(400, "缺少 config 对象");
    };
    let config = match parse_config(block) {
        Ok(config) => config,
        Err(message) => return errors::management_error(400, message),
    };
    if !config::set_assistant_config(config.clone()) {
        logging::log(
            "[Config]",
            "⚠️  AI 助手设置写入失败（本次运行内仍生效）",
        );
    }
    // 回给前端的形态与 GET 完全一致（展开过的工具表）——「存了什么」与
    // 「界面看到什么」用同一个序列化口，免得前端保存后自己那那份与后端分叉
    ok_json(json!({ "config": config::assistant_config().status_value() }))
}

/// 解析并校验设置体（校验失败给人话，不静默丢弃）
fn parse_config(block: &Map<String, Value>) -> Result<AssistantConfig, String> {
    let model = match block.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => {
            let id = text.trim();
            if id.is_empty() {
                None
            } else {
                if id.chars().count() > MAX_MODEL_CHARS {
                    return Err("模型名过长".to_string());
                }
                Some(id.to_string())
            }
        }
        Some(_) => return Err("model 必须是字符串或 null".to_string()),
    };
    let system_prompt = match block.get("systemPrompt") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => {
            if text.chars().count() > MAX_SYSTEM_PROMPT_CHARS {
                return Err(format!("系统提示词最长 {MAX_SYSTEM_PROMPT_CHARS} 字"));
            }
            text.clone()
        }
        Some(_) => return Err("systemPrompt 必须是字符串".to_string()),
    };
    let tools = match block.get("tools") {
        None | Some(Value::Null) => Default::default(),
        Some(Value::Object(tools)) => tools
            .iter()
            .filter_map(|(id, on)| on.as_bool().map(|on| (id.clone(), on)))
            .collect(),
        Some(_) => return Err("tools 必须是对象".to_string()),
    };
    Ok(AssistantConfig {
        model,
        system_prompt_enabled: block
            .get("systemPromptEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        system_prompt,
        tools,
    })
}

/* ─── 对话（SSE）────────────────────────────────────────── */

/// POST /api/assistant/chat
pub async fn chat(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(messages) = payload.get("messages").and_then(Value::as_array).cloned() else {
        return errors::management_error(400, "缺少 messages 数组");
    };
    if messages.is_empty() {
        return errors::management_error(400, "messages 不能为空");
    }
    if messages.len() > MAX_MESSAGES {
        return errors::management_error(400, format!("messages 最多 {MAX_MESSAGES} 条"));
    }
    // 本次请求点名的模型优先；没点名用设置里的。两者都没有时说清下一步 ——
    // 「去设置里选一个」比「模型未指定」这种网关腔有用得多。
    // 这里的 model 是**对外模型名**（/v1/models 的 data[].id，映射名也算），
    // 路由交给公共管道解析 —— 同名在多家时按账号优先级主备，与普通客户端一致。
    let config = config::assistant_config();
    let model = match model_from(&payload) {
        Ok(Some(model)) => model,
        Ok(None) => match config.model.clone() {
            Some(model) => model,
            None => {
                return errors::management_error(
                    400,
                    "还没有选择对话模型：去「设置 → AI 助手」里选一个，或先在模型管理页确认有已启用的模型",
                )
            }
        },
        Err(message) => return errors::management_error(400, message),
    };
    let tools = match payload.get("tools") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(tools)) => {
            if tools.len() > MAX_TOOLS {
                return errors::management_error(400, format!("tools 最多 {MAX_TOOLS} 个"));
            }
            tools.clone()
        }
        Some(_) => return errors::management_error(400, "tools 必须是数组"),
    };

    // 系统提示词：开关 × 工具是否为空的四态口径全在 config 侧（含「一只工具都
    // 没勾就不注入」这条产品约定）；这里只负责把它摆到 messages 最前面
    let system = config.system_prompt(!tools.is_empty());
    let mut outbound_messages = Vec::with_capacity(messages.len() + 1);
    if let Some(text) = system {
        outbound_messages.push(json!({ "role": "system", "content": text }));
    }
    outbound_messages.extend(messages);

    let mut upstream = json!({
        "model": model,
        "messages": outbound_messages,
        // 助手固定流式：工具调用与正文都要边到边渲染，非流式对对话界面没有意义
        "stream": true,
    });
    if !tools.is_empty() {
        upstream["tools"] = Value::Array(tools);
    }

    // 模型路由：对外名 → 上游名（默认值 / 映射别名 / 目录校验与 /v1/* 同源）
    let requested_model = match pipeline::resolve_model(&state, &mut upstream, None) {
        Ok(name) => name,
        Err(error) => return errors::management_error(error.http_status().as_u16() as i32, error.message),
    };

    let started_at = logging::now_ms();
    let telemetry = Arc::new(RequestTelemetry::with_id());
    let telemetry_id = telemetry.snapshot().id;
    // 进行中行：与真实请求同一时点。`is_test = true` —— 报表排除、请求日志标记
    // （与模型测试同一口径，见模块头）；下游模型名记用户点名的那个对外名
    state.request_stats().record_started(
        &telemetry_id,
        started_at,
        &requested_model,
        &model,
        "",
        "",
        true,
    );
    if let Some(token) = cancellation::register(&telemetry_id) {
        telemetry.set_cancel_token(token);
    }
    let mut guard = DisconnectGuard::new(state.request_stats(), telemetry_id.clone());

    let outcome = state
        .upstream()
        .forward(ForwardRequest {
            body: upstream,
            stream: true,
            // 不去重：对话不是「同 body 的快速重试」，同一句再问一次就该再发一次
            dedupe_key: String::new(),
            // 空头：助手不是某个客户端发来的，也不该把面板的管理头透传给上游
            client_headers: HeaderMap::new(),
            telemetry: telemetry.clone(),
            // **不钉提供商**：对话模型是对外名，路由语义与 /v1/chat/completions
            // 完全一致 —— 同名在多家时按账号优先级主备，失败自动落下一家
            allowed_providers: None,
            pinned_account: None,
            // 尊重模型启停：助手用的是「已启用的模型」，用户关掉的模型不该
            // 因为助手而继续消耗额度（与生产请求同一道门禁）
            ignore_model_gate: false,
            proxy_override: None,
        })
        .await;

    match outcome {
        Ok(ForwardOutcome::Stream { status, stream: source }) => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
            let context = RecordContext {
                stats: state.request_stats(),
                telemetry,
                started_at,
                model: requested_model,
                client_model: model.clone(),
                client_reasoning: String::new(),
                status: i64::from(status.as_u16()),
                raw_request: None,
                raw_response: None,
                is_test: true,
                client: String::new(),
            };
            // 记账移交给响应流（它与流的生命周期同长）；守卫不再兜底
            guard.handoff();
            sse_response(
                status,
                Box::new(RecordingStream::with_terminals(
                    source,
                    context,
                    pipeline::terminal::CHAT,
                )),
            )
            .into_response()
        }
        // 固定要求了流式，聚合体这条理论上到不了；真到了就按 JSON 原样回，
        // 前端对非 SSE 响应也有一套降级（把整段当一条 assistant 消息）
        Ok(ForwardOutcome::Completion { body }) => {
            let raw_response = pipeline::capture_raw_json(&body);
            pipeline::record_entry(
                &RecordContext {
                    stats: state.request_stats(),
                    telemetry,
                    started_at,
                    model: requested_model,
                    client_model: model.clone(),
                    client_reasoning: String::new(),
                    status: 200,
                    raw_request: None,
                    raw_response,
                    is_test: true,
                    client: String::new(),
                },
                None,
            );
            guard.complete();
            json_response(body)
        }
        Err(error) => {
            let status = i64::from(error.http_status().as_u16());
            let message = error.message.clone();
            pipeline::record_entry(
                &RecordContext {
                    stats: state.request_stats(),
                    telemetry,
                    started_at,
                    model: requested_model,
                    client_model: model.clone(),
                    client_reasoning: String::new(),
                    status,
                    raw_request: None,
                    raw_response: None,
                    is_test: true,
                    client: String::new(),
                },
                Some(message.clone()),
            );
            guard.complete();
            errors::management_error(status as i32, message)
        }
    }
}

/// 从请求体读本次点名的模型：**对外模型名**（字符串；缺失 / null → None）。
///
/// 兼容旧版前端发来的 `{provider, id}` 对象 —— 取 `id`，provider 不再参与路由
/// （对话模型的语义已与 /v1/models 对齐，同名在多家按账号优先级）。
fn model_from(payload: &Value) -> Result<Option<String>, String> {
    match payload.get("model") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let id = text.trim();
            if id.is_empty() {
                return Err("model 不能为空".to_string());
            }
            Ok(Some(id.to_string()))
        }
        Some(Value::Object(model)) => {
            let id = model.get("id").and_then(Value::as_str).unwrap_or("").trim();
            if id.is_empty() {
                return Err("model.id 不能为空".to_string());
            }
            Ok(Some(id.to_string()))
        }
        Some(_) => Err("model 必须是字符串".to_string()),
    }
}
