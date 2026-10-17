//! 开发者角色兼容开关：`GET/PUT /api/developer-role-compat`。
//!
//! ── 这个开关做什么 ──────────────────────────────────────────
//! 开启后转发层在每次出站前把请求 `messages` 里 `role: "developer"` 的消息
//! 改写为 `role: "system"`（见 `core::upstream::payload` 的 send_body 角色归一
//! 层）：OpenAI 新版 API 用 developer 取代 system 承载指令，而部分 OpenAI
//! 兼容上游（含自定义提供商的透传路径）不认识这个角色，会直接 400。
//!
//! ── 为什么只有一个开关 ──────────────────────────────────────
//! 转换是**语义无损**的固定规则（developer 与 system 在 OpenAI 语义上等价，
//! OpenAI 自己的迁移指南也是这么映射的），没有词表、没有作用范围可配；
//! 形态与 `/api/sanitize`、`/api/debug` 一致 —— GET 读、PUT 写、响应体就是
//! 新状态，设置页那排单开关的读写代码共用一套模式。
//!
//! ── 写盘位置 ────────────────────────────────────────────────
//! `config` 的 `developerRoleCompat` 键（统一库 `kv` 表里的配置行），默认开。
//! 转发层逐请求读快照，改完下一个请求立即生效，不重启进程。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/developer-role-compat —— 当前开关状态
pub async fn get_role_compat(State(_state): State<ServerState>) -> Response {
    ok_json(role_compat_json())
}

/// PUT /api/developer-role-compat —— body `{developerRoleCompat: true|false}`
///
/// 写配置，下一个请求立即生效（转发层逐请求读快照，不重启进程）。
pub async fn put_role_compat(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let key = config::KEY_DEVELOPER_ROLE_COMPAT;
    let Some(value) = object.get(key) else {
        return errors::management_error(400, format!("缺少 {key}"));
    };
    let Some(enabled) = value.as_bool() else {
        return errors::management_error(400, format!("{key} 必须是 true 或 false"));
    };
    if !config::set_developer_role_compat(enabled) {
        logging::log(
            "[Config]",
            "⚠️  开发者角色兼容设置写入失败，本次运行内仍生效",
        );
    }
    logging::log(
        "[Config]",
        if enabled {
            "开发者角色兼容已开启：转发前把 developer 角色消息改写为 system"
        } else {
            "开发者角色兼容已关闭：developer 角色消息将原样发往上游"
        },
    );
    ok_json(role_compat_json())
}

/// 开关状态（GET 与 PUT 共用，前端直接用响应刷新界面）
fn role_compat_json() -> Value {
    json!({
        config::KEY_DEVELOPER_ROLE_COMPAT: config::current().developer_role_compat(),
    })
}
