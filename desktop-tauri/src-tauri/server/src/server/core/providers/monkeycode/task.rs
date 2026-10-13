//! MonkeyCode 建任务与输入侧翻译（会话转发的第一步）。
//!
//! ── 输入侧：OpenAI messages → 上游 prompt（参考 `api-routes.ts`）──
//! 与 `api-routes.ts::messagesToPrompt` 逐条对齐：
//!   - 第一条**非空** `system` 消息抽出来，单独走建任务的 `system_prompt`
//!     字段（参考是 `messages.find(role === "system")`，只取一条）；
//!   - 其余消息按角色加前缀：user → `[User]\n…`、assistant →
//!     `[Assistant]\n…`、其它角色原样，段间以 `\n\n` 连接；
//!   - `tools` / `tool_choice` **不透传**：MonkeyCode 的 Agent 在 VM 内自带
//!     工具循环（opencode / codex / claude），建任务接口没有接收客户端工具表的
//!     字段，参考实现同样忽略它们。不为此报错 —— 客户端带工具表是常态。
//!   - 多模态 `content` 数组只取 text / input_text 部件拼接（参考实现直接当
//!     字符串用，数组会变成 `[object Object]`；这里至少把文本挑出来）。
//!
//! ── 建任务请求体（逐字对照 `task-runner.ts::createTask`）──────────
//! ```jsonc
//! { "content": <prompt>, "host_id": "public_host", "image_id": <账号字段>,
//!   "model_id": <目录 upstreamId>, "cli_name": "opencode|codex|claude",
//!   "resource": {"core": 1, "memory": 1073741824, "life": 3600},
//!   "repo": {"repo_url": "", "branch": "master", "repo_filename": "", "zip_url": ""} }
//! ```
//! `system_prompt` 仅在客户端带了 system 消息时插入（参考的条件插入）。
//! `resource.life = 3600` 与任务流 1 小时总超时同一口径（参考注释原文：
//! "matches resource.life"）。
//!
//! ── 起始 user-input 的格式（本家一个容易踩的点）─────────────────
//! 参考的 TS 实现发**纯文本**；`docs/protocol/llm-protocol-complete.md` §5.1
//! 把「`{"content": base64, "attachments": []}` 的 JSON 串」列为**推荐**的新
//! 上行格式、纯文本为「仍兼容」。后端 `parseUserInputData()` 按「存储格式 →
//! 新格式 → 纯文本」三级解析。这里发推荐的新格式；若上游版本只认纯文本，
//! 改 `user_input_payload` 一处即可。
//!
//! ── 建任务失败怎么变 GatewayError ───────────────────────────────
//!   - 传输层（连不上 / 超时）→ 502 / 504；
//!   - HTTP 401/403 → 401（登录态失效，重新粘贴 session）；
//!   - HTTP 402 → 402（额度不足）；404 → 404（模型 / 镜像不存在）；
//!     409 → 409（冲突）；429 → 429（限流）；其余非 2xx 原状态码透传
//!     （5xx 归一成 502 —— 那是上游故障，可重试）；
//!   - HTTP 200 但 `code != 0`（参考 `task-runner.ts:92-95` 明确处理）：
//!     `10811` → 409（同账号已有运行中的任务）、`40100` → 401、
//!     `40002/40003/40004` → 403（账号密码错 / 封禁 / 未激活）、
//!     `50000` → 502（可重试）、其余 → 502 并把业务码带进 `upstream_code`。
//!     码表来源 `docs/10-appendices/02-error-codes.md`。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic；不持任何锁。

use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use crate::server::core::egress;
use crate::server::core::providers::kind_id;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::client;
use super::credentials::MonkeyCodeCredentials;
use super::endpoints;
use super::models;
use super::region::Region;

/// 建任务请求超时（目录 / 校验同档的 20 秒太紧 —— 建任务要等上游调度，
/// 参考实现没设超时；这里给 30 秒，上游挂住时不至于拖死网关）
const CREATE_TIMEOUT_MS: u64 = 30_000;

/// 起始用户消息：`content`（建任务 content，与 WS `user-input` 同源）+
/// 可选的 `system_prompt`（单独字段，不进 content）。
pub struct TaskPrompt {
    pub content: String,
    pub system_prompt: Option<String>,
}

/// OpenAI messages → 上游 prompt（判据见模块头）。
pub fn build_prompt(messages: Option<&Value>) -> Result<TaskPrompt, GatewayError> {
    let Some(Value::Array(items)) = messages else {
        return Err(GatewayError::bad_request("请求缺少 messages 数组"));
    };
    let mut system_prompt: Option<String> = None;
    let mut parts: Vec<String> = Vec::new();
    for message in items {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let text = content_text(message).unwrap_or_default();
        if role == "system" {
            if system_prompt.is_none() && !text.trim().is_empty() {
                system_prompt = Some(text);
            }
            continue;
        }
        if text.trim().is_empty() {
            continue;
        }
        match role {
            "user" => parts.push(format!("[User]\n{text}")),
            "assistant" => parts.push(format!("[Assistant]\n{text}")),
            _ => parts.push(text),
        }
    }
    let content = parts.join("\n\n");
    if content.trim().is_empty() {
        return Err(GatewayError::bad_request(
            "messages 里没有可发送的文本内容：MonkeyCode 只接收文本提示词（图片 / 文件附件不在本期范围）",
        ));
    }
    Ok(TaskPrompt { content, system_prompt })
}

/// 取一条消息的可读文本（content 字符串，或部件数组里的 text / input_text）。
fn content_text(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                let text = part
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.get("input_text").and_then(Value::as_str));
                if let Some(text) = text {
                    out.push_str(text);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

/// 建任务请求体（逐字对照参考，见模块头；`region` 只用于取 `HOST_ID` 覆盖）。
pub fn build_body(
    region: Region,
    target: &models::TaskTarget,
    prompt: &TaskPrompt,
    image_id: &str,
) -> Value {
    let mut body = Map::new();
    body.insert("content".to_string(), Value::String(prompt.content.clone()));
    body.insert(
        "host_id".to_string(),
        Value::String(endpoints::host_id(region)),
    );
    body.insert("image_id".to_string(), Value::String(image_id.to_string()));
    body.insert("model_id".to_string(), Value::String(target.model_id.clone()));
    body.insert("cli_name".to_string(), Value::String(target.cli_name.clone()));
    body.insert(
        "resource".to_string(),
        json!({ "core": 1, "memory": 1073741824, "life": 3600 }),
    );
    body.insert(
        "repo".to_string(),
        json!({
            "repo_url": "",
            "branch": "master",
            "repo_filename": "",
            "zip_url": "",
        }),
    );
    if let Some(system) = prompt
        .system_prompt
        .as_ref()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
    {
        body.insert("system_prompt".to_string(), Value::String(system.to_string()));
    }
    Value::Object(body)
}

/// 起始 `user-input` 的内容：新上行格式 `{"content": "<base64>", "attachments": []}`
/// 的 **JSON 串**（`data` 字段的值；见模块头）。
pub fn user_input_payload(prompt: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(prompt.as_bytes());
    json!({ "content": encoded, "attachments": [] }).to_string()
}

/// 创建任务，返回上游 task id（UUID）。
pub async fn create_task(
    region: Region,
    credentials: &MonkeyCodeCredentials,
    target: &models::TaskTarget,
    prompt: &TaskPrompt,
    proxy: Option<&ResolvedProxy>,
    telemetry: &RequestTelemetry,
) -> Result<String, GatewayError> {
    let body = build_body(region, target, prompt, &credentials.image_id);
    let url = endpoints::task_create_url(region);
    let headers = endpoints::authed_headers(region, &credentials.session);
    // ── 调试模式：抓一份即将发出的原始报文 ────────────────────────
    // 与各家同一时机（请求体已定稿、即将发送）；开关关着时 capture 为 None，
    // 整段不执行。任务流（WS）本身不是 HTTP 往返，没有可采集的请求 / 响应体，
    // 因此本家只有这一次建任务调用进调试面板。
    let capture = telemetry.capture();
    if let Some(capture) = capture.as_deref() {
        capture.reset_request(&url, kind_id(region.kind()), &headers, &body);
    }
    let client = egress::client_for(proxy);
    let mut builder = client.post(&url).header("Accept", "application/json");
    for (name, value) in &headers {
        builder = builder.header(name.as_str(), value);
    }
    let response = builder
        .json(&body)
        .timeout(Duration::from_millis(CREATE_TIMEOUT_MS))
        .send()
        .await
        .map_err(|error| transport_error("MonkeyCode 建任务请求", error))?;
    let status = response.status().as_u16();
    if let Some(capture) = capture.as_deref() {
        capture.attach_response(status, response.headers());
    }
    let text = response.text().await.map_err(|error| {
        GatewayError::with_status(
            502,
            format!(
                "MonkeyCode 建任务响应读取失败：{}",
                egress::describe_error_detail(&error)
            ),
        )
    })?;
    if let Some(capture) = capture.as_deref() {
        capture.push(text.as_bytes());
    }
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        return Err(status_error(status, &payload, &text));
    }
    check_business_code(&payload)?;
    let task_id = task_id_of(&payload)?;
    logging::verbose(
        "[MonkeyCode]",
        &format!(
            "建任务成功 model={} cli={} task={} promptLen={} systemPrompt={} region={}",
            target.model,
            target.cli_name,
            task_id,
            prompt.content.chars().count(),
            if prompt.system_prompt.is_some() { "有" } else { "无" },
            region.id(),
        ),
    );
    Ok(task_id)
}

/// 传输层错误 → 网关错误（超时 504、其余 502；与各家同款文案）。
fn transport_error(what: &str, error: reqwest::Error) -> GatewayError {
    if error.is_timeout() {
        GatewayError::with_status(504, format!("{what}超时，请稍后重试"))
    } else {
        GatewayError::with_status(
            502,
            format!("{what}失败：{}", egress::describe_error_detail(&error)),
        )
    }
}

/// HTTP 非 2xx → 网关错误（码表见模块头）。
fn status_error(status: u16, payload: &Value, text: &str) -> GatewayError {
    let detail = {
        let upstream = client::upstream_message(payload);
        if upstream.is_empty() {
            text.trim().chars().take(200).collect::<String>()
        } else {
            upstream
        }
    };
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!("：{detail}")
    };
    match status {
        401 | 403 => GatewayError::with_status(
            401,
            format!(
                "MonkeyCode 登录态已失效或无权访问（HTTP {status}）：请在账号页重新粘贴 session{suffix}"
            ),
        ),
        402 => GatewayError::with_status(
            402,
            format!("MonkeyCode 账户额度不足（HTTP 402），请确认订阅 / 余额后再试{suffix}"),
        ),
        404 => GatewayError::with_status(
            404,
            format!("MonkeyCode 模型或镜像不存在（HTTP 404），请确认模型清单与账号的 image_id 仍然有效{suffix}"),
        ),
        409 => GatewayError::with_status(
            409,
            format!("MonkeyCode 建任务冲突（HTTP 409，可能已有运行中的任务）{suffix}"),
        ),
        429 => GatewayError::with_status(
            429,
            format!("MonkeyCode 请求过于频繁（HTTP 429），请稍后重试{suffix}"),
        ),
        // 5xx 归 502：上游故障是**可重试**的，与 4xx 的参数 / 权限问题分开
        _ if status >= 500 => GatewayError::with_status(
            502,
            format!("MonkeyCode 建任务返回 HTTP {status}（上游故障，可稍后重试）{suffix}"),
        ),
        _ => GatewayError::with_status(
            status as i32,
            format!("MonkeyCode 建任务返回 HTTP {status}{suffix}"),
        ),
    }
}

/// HTTP 200 + 业务码（`code != 0`）→ 网关错误（码表见模块头）。
fn check_business_code(payload: &Value) -> Result<(), GatewayError> {
    let Some(code) = client::business_code(payload) else {
        return Ok(());
    };
    if code == 0 {
        return Ok(());
    }
    let message = client::upstream_message(payload);
    let suffix = if message.is_empty() {
        String::new()
    } else {
        format!("：{message}")
    };
    let error = match code {
        40100 => GatewayError::with_status(
            401,
            format!("MonkeyCode 登录态已失效（code 40100）：请在账号页重新粘贴 session{suffix}"),
        ),
        40002 | 40003 | 40004 => GatewayError::with_status(
            403,
            format!("MonkeyCode 账号不可用（code {code}：密码错误 / 被封禁 / 未激活）{suffix}"),
        ),
        10811 => GatewayError::with_status(
            409,
            format!("MonkeyCode 该账号已有正在运行的任务（code 10811），请等它结束或先停止它{suffix}"),
        ),
        50000 => GatewayError::with_status(
            502,
            format!("MonkeyCode 上游服务端错误（code 50000，可稍后重试）{suffix}"),
        ),
        _ => GatewayError::with_status(
            502,
            format!("MonkeyCode 建任务失败（业务码 {code}）{suffix}"),
        ),
    };
    Err(error.upstream_code(code))
}

/// 从建任务响应里取 task id（参考 `data.id || data.task_id`）。
fn task_id_of(payload: &Value) -> Result<String, GatewayError> {
    let data = payload.get("data").unwrap_or(payload);
    for key in ["id", "task_id", "ID"] {
        let value = data
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    Err(GatewayError::with_status(
        502,
        "MonkeyCode 建任务响应里没有 task id（上游响应形态变化？）",
    ))
}
