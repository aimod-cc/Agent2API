//! 第三方模型回答完自动折叠过程开关：`GET/PUT /api/reasoning-collapse`。
//!
//! ── 这个开关做什么 ──────────────────────────────────────────
//! 仅对严格识别为 Codex 的请求补阶段标记；显示规则与来源声明不参与判断。
//! 正常结束且没有工具调用的非空回答补 `final_answer`，工具调用前的说明补
//! `commentary`。错误、截断与缺少结束证据时不补最终答案标记。
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
