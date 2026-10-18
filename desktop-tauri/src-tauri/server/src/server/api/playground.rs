//! 试炼场 API：`POST /api/playground/chat`。
//!
//! ── 为什么不是让前端直连 `/v1/chat/completions`（这是本模块存在的全部理由）──
//! 桌面壳的页面来自 Tauri 自定义协议（`tauri://localhost`），前端 fetch 到
//! `http://127.0.0.1:{port}/v1/*` 是**跨源**请求，而网关面**默认不应答 CORS**
//! （`KEY_CORS_ENABLED` 的默认值是 `false`，且它是一道安全边界 —— 开着 `*`
//! 等于把「转发上游、消耗额度」的能力交给任何网页）。
//! 让试炼场直连 `/v1/*`，等于要求用户先打开一个与它无关的安全开关；
//! **实测过**：CORS 关掉后那条路直接 `TypeError: Failed to fetch`，
//! 而一个全新安装的用户默认就是关的。
//! 所以试炼场也挂到面板面（`/api/*`，无条件 CORS），与 AI 助手同一个处置
//! —— 那段论证原本就写在 `api::assistant` 的模块头里。
//!
//! ── 那它凭什么代表「真实客户端能不能用」──────────────────────
//! 因为它与 `/v1/chat/completions` **共用同一条转发链路**
//! （`UpstreamService::forward`）：token 刷新、429 换号、冷却、出站脱敏、
//! 协议翻译、模型路由（对外名 → 上游名）全部是同一条代码。
//! 这一层只做三件事，与真实客户端能观察到的差异**为零**：
//!   ① 透传 `model` / `messages` / `temperature` / `max_tokens`；
//!   ② 不注入任何系统提示词（**与助手的分界线**：助手注入了管理助手人设，
//!      这里不注入 —— 试炼场要观察的是「客户端原样发这条请求，上游怎么答」）；
//!   ③ 把 SSE 原样流回前端，由前端解析正文 / 思考过程 / usage。
//!
//! ── 与助手的差别（两者并存而不重复）────────────────────────
//!   · 助手：注入系统提示词 + 带 tools（能改配置）+ 固定流式；
//!   · 试炼场：不注入、不带 tools、**可选采样参数**、把 usage 与耗时如实回传。
//! 前者是「能动手的运维入口」，后者是「只读的连通性验证台」。
//!
//! ── 记账口径 ────────────────────────────────────────────────
//! 与助手 / 模型测试完全一致：进请求日志（用户在「请求日志」里看得见每一次
//! 消耗），`is_test = true` 使报表聚合排除它 —— 人工反复点「发送」产生的
//! 样本混进趋势图会把真实流量读歪。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::server::core::upstream::cancellation;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::{ForwardOutcome, ForwardRequest};
use crate::server::errors;
use crate::server::http::parse_body;
use crate::server::logging;
use crate::server::ServerState;

use super::disconnect_guard::DisconnectGuard;
use super::pipeline::{self, json_response, RecordContext, RecordingStream, sse_response};

/// 对外模型名的长度上限（与助手 / 模型测试同一量级）
const MAX_MODEL_CHARS: usize = 128;
/// messages 条数上限：与助手同值（防前端 bug 把上下文堆爆，不是防用户）
const MAX_MESSAGES: usize = 200;
/// 单条消息正文的字符上限。
///
/// 比助手的提示词上限更宽（试炼场常用来贴长文验证上下文能力），但仍设上界
/// —— 一个失控的粘贴不该把网关内存打满。2 万字符约等于一篇长文。
const MAX_MESSAGE_CHARS: usize = 20_000;

/// POST /api/playground/chat
pub async fn chat(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return errors::management_error(400, error.message),
    };

    // ── model ──
    let model = match payload.get("model").and_then(Value::as_str).map(str::trim) {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => return errors::management_error(400, "缺少 model（请先选择一个模型）"),
    };
    if model.chars().count() > MAX_MODEL_CHARS {
        return errors::management_error(400, format!("model 最多 {MAX_MODEL_CHARS} 个字符"));
    }

    // ── messages ──
    let Some(messages) = payload.get("messages").and_then(Value::as_array) else {
        return errors::management_error(400, "缺少 messages 数组");
    };
    if messages.is_empty() {
        return errors::management_error(400, "messages 不能为空");
    }
    if messages.len() > MAX_MESSAGES {
        return errors::management_error(400, format!("messages 最多 {MAX_MESSAGES} 条"));
    }
    for message in messages {
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        if content.chars().count() > MAX_MESSAGE_CHARS {
            return errors::management_error(
                400,
                format!("单条消息最多 {MAX_MESSAGE_CHARS} 个字符"),
            );
        }
    }

    // ── 组装上游请求 ──
    // **逐字透传** messages：不拼系统提示词、不改 role、不裁内容 ——
    // 这一页要看的就是「原样发出去会怎样」（与助手的分界线见模块头）。
    let mut upstream = json!({
        "model": model,
        // 固定流式：这一页的核心读数（首字节耗时、逐字渲染、思考过程）都要求流式
        "stream": true,
        "messages": messages,
    });

    // ── stream_options：**默认不传，与真实客户端一致** ──────────────
    // 起初想默认打开 `include_usage`（这样流式也能拿到 token 用量），但那会让
    // 试炼场的请求体与客户端**不一样** —— 某些上游不认这个字段会直接 400，
    // 于是试炼场报一个「客户端根本不会遇到」的错，这一页的可信度就没了。
    // 现在的口径：
    //   · 客户端默认（不传）→ 拿不到 usage，统计条按「—」显示（如实）；
    //   · 用户显式勾「要用量读数」→ 传 `include_usage`，**并把风险说在前面**
    //     （界面上写着「部分上游不支持，可能报错」）。
    // 宁可少给一个读数，也不给一个与客户端行为不符的请求体。
    if payload.get("include_usage").and_then(Value::as_bool) == Some(true) {
        upstream["stream_options"] = json!({ "include_usage": true });
    }

    // ── 采样参数（可选，缺省一律不传 —— 与 OpenAI SDK 的「不传就用上游默认」一致）──
    // temperature：合法域 [0, 2]（OpenAI 规范）；越界直接拒，**不静默夹取** ——
    // 夹取值会让「我设了 3 却只得到 2 的行为」变成无法解释的现象。
    if let Some(value) = payload.get("temperature") {
        let Some(number) = value.as_f64() else {
            return errors::management_error(400, "temperature 必须是数字");
        };
        if !(0.0..=2.0).contains(&number) {
            return errors::management_error(400, "temperature 必须在 0–2 之间");
        }
        upstream["temperature"] = json!(number);
    }
    // max_tokens：正整数；上限取 1_000_000（比任何现有模型的最大输出都宽，
    // 具体能到多少由上游自己判 —— 这里只挡明显是笔误的值）
    if let Some(value) = payload.get("max_tokens") {
        let Some(number) = value.as_i64() else {
            return errors::management_error(400, "max_tokens 必须是整数");
        };
        if number <= 0 || number > 1_000_000 {
            return errors::management_error(400, "max_tokens 必须在 1–1000000 之间");
        }
        upstream["max_tokens"] = json!(number);
    }

    // 模型路由：对外名 → 上游名（默认值 / 映射别名 / 目录校验与 /v1/* 同源）
    let requested_model = match pipeline::resolve_model(&state, &mut upstream, None) {
        Ok(name) => name,
        Err(error) => {
            return errors::management_error(error.http_status().as_u16() as i32, error.message)
        }
    };

    let started_at = logging::now_ms();
    let telemetry = Arc::new(RequestTelemetry::with_id());
    let telemetry_id = telemetry.snapshot().id;
    // 进行中行：与真实请求同一时点。`is_test = true` —— 报表排除、请求日志标记
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
            // 不去重：同一句话手动再发一次就该再发一次（与助手同款理由）
            dedupe_key: String::new(),
            // 空头：试炼场不是某个真实客户端发来的（虽然它模拟的是客户端行为），
            // 把面板的管理头透传给上游毫无意义
            client_headers: HeaderMap::new(),
            telemetry: telemetry.clone(),
            // **不钉提供商**：与 /v1/chat/completions 的路由语义完全一致 ——
            // 同名在多家时按账号优先级主备，失败自动落下一家。
            // 这一条正是「结论能代表真实客户端」的关键。
            allowed_providers: None,
            pinned_account: None,
            // 尊重模型启停：用户关掉的模型不该因为试炼场而继续消耗额度
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
                client_model: model,
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
        // 固定要求了流式，聚合体这条理论上到不了；真到了就按 JSON 原样回
        Ok(ForwardOutcome::Completion { body }) => {
            let raw_response = pipeline::capture_raw_json(&body);
            pipeline::record_entry(
                &RecordContext {
                    stats: state.request_stats(),
                    telemetry,
                    started_at,
                    model: requested_model,
                    client_model: model,
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
                    client_model: model,
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
