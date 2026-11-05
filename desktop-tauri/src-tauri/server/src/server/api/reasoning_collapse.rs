//! 第三方模型回答完自动折叠过程开关：`GET/PUT /api/reasoning-collapse`。
//!
//! ── 这个开关做什么 ──────────────────────────────────────────
//! 开启后，网关在重建 Responses 事件流时给助手消息补 `phase` 标记：本轮没有
//! 工具调用的最后一条消息补 `final_answer`，之前的正文与工具调用之外的过程
//! 消息补 `commentary`（见 `core::protocol::responses`）。Codex 桌面端只在收到
//! `phase: "final_answer"` 时，才把这一轮的推理、命令与过程消息收进可折叠的
//! 「已处理 X 分钟」；第三方模型不带这个字段，整轮过程因此一直展开。
//!
//! ── 为什么只有一个开关 ──────────────────────────────────────
//! 判定规则是固定的（看本轮有没有客户端工具调用），没有阈值与作用范围可配；
//! 形态与 `/api/developer-role-compat`、`/api/keepalive-strip` 一致 ——
//! GET 读、PUT 写、响应体就是新状态，设置页那排单开关共用一套读写模式。
//!
//! ── 写盘位置 ────────────────────────────────────────────────
//! `config` 的 `thirdPartyReasoningCollapse` 键（统一库 `kv` 表里的配置行），
//! 默认关。协议重建层逐请求读快照，改完下一个请求立即生效，不重启进程。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/reasoning-collapse —— 当前开关状态
pub async fn get_reasoning_collapse(State(_state): State<ServerState>) -> Response {
    ok_json(reasoning_collapse_json())
}

/// PUT /api/reasoning-collapse —— body `{thirdPartyReasoningCollapse: true|false}`
///
/// 写配置，下一个请求立即生效（协议重建层逐请求读快照，不重启进程）。
pub async fn put_reasoning_collapse(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let key = config::KEY_THIRD_PARTY_REASONING_COLLAPSE;
    let Some(value) = object.get(key) else {
        return errors::management_error(400, format!("缺少 {key}"));
    };
    let Some(enabled) = value.as_bool() else {
        return errors::management_error(400, format!("{key} 必须是 true 或 false"));
    };
    if !config::set_third_party_reasoning_collapse(enabled) {
        logging::log(
            "[Config]",
            "⚠️  第三方模型回答完自动折叠过程设置写入失败，本次运行内仍生效",
        );
    }
    logging::log(
        "[Config]",
        if enabled {
            "第三方模型回答完自动折叠过程已开启：Responses 助手消息将补 final_answer / commentary 阶段标记"
        } else {
            "第三方模型回答完自动折叠过程已关闭：Responses 助手消息不再补阶段标记"
        },
    );
    ok_json(reasoning_collapse_json())
}

/// 开关状态（GET 与 PUT 共用，前端直接用响应刷新界面）
fn reasoning_collapse_json() -> Value {
    json!({
        config::KEY_THIRD_PARTY_REASONING_COLLAPSE: config::current().third_party_reasoning_collapse(),
    })
}
