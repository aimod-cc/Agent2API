//! GET/PUT /api/schedule —— 账号调度模式与会话粘性设置。
//!
//! ── 这两项管什么 ────────────────────────────────────────────
//!   - `scheduleMode`：转发时在候选账号里**按什么顺序挑**。三选一：
//!     `priority` 严格优先级（默认，与改造前逐字相同）/ `provider` 同一
//!     提供商内轮询（家之间仍按优先级）/ `global` 全部账号一条环轮询。
//!     各模式的语义见 `config::ScheduleMode` 的说明。
//!   - `sessionSticky`：会话粘性开关（默认关）。开启后优先使用 trace 最近
//!     选中的账号，其次 thread 最近选中的账号，粘性账号失败后按正常顺序回退。
//!     提取链与记录口径见 `core::session_sticky` 的模块头。
//!
//! 两项存在配置库（键名契约见 `config.rs` 的 `KEY_SCHEDULE_MODE` /
//! `KEY_SESSION_STICKY`），由设置页「调度」分类经 HTTP 桥读写 —— 与
//! /api/retry 同一模式：独立端点、允许部分字段、返回生效后的全量值、
//! 无副作用（不重启进程）。保存后对下一个请求立即生效（转发层每轮选路
//! 读内存快照）。
//!
//! ── 模式为什么按白名单校验而不是「不认识就回落」────────────────
//! 读侧（手改库）的容错口径是回落默认值（不打扰），但走接口的值是用户
//! **明确选**的：给一个拼错的模式名却回 200，界面会显示成「已切换」而
//! 实际仍是严格优先级 —— 这类静默分歧排障时最贵，所以这里 400 点名
//! 合法取值。两侧分工与 `/api/retention` 的「接口 400、手改回落」同款。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::{
    self, ScheduleMode, SchedulePatch, ScheduleSettings, KEY_SCHEDULE_MODE, KEY_SESSION_STICKY,
};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/schedule
pub async fn get_schedule(State(_state): State<ServerState>) -> Response {
    ok_json(schedule_json(config::schedule_settings()))
}

/// PUT /api/schedule —— body `{scheduleMode?, sessionSticky?}`
///
/// 允许部分字段（未出现的项保持原值，null 同义）。校验通过后：写配置库
/// → 返回**生效后**的值（前端直接用响应刷新界面，不必再 GET 一次）。
pub async fn put_schedule(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };

    // 校验与写盘分两步（与 put_queue 同一顺序）：两项里有一项非法时整体
    // 不落盘，避免出现「模式改了、粘性没改」的半套设置
    let mut patch = SchedulePatch::default();
    let mut updated = false;

    if let Some(value) = object.get(KEY_SCHEDULE_MODE) {
        // null 视为「这一项不改」：前端整份回传时未填项常见为 null
        if !value.is_null() {
            let Some(text) = value.as_str() else {
                return errors::management_error(
                    400,
                    format!("{KEY_SCHEDULE_MODE} 必须是字符串（收到: {value}）"),
                );
            };
            let Some(mode) = ScheduleMode::parse(text) else {
                return errors::management_error(
                    400,
                    format!(
                        "{KEY_SCHEDULE_MODE} 只支持 priority / provider / global（收到: {text}）"
                    ),
                );
            };
            patch.mode = Some(mode);
            updated = true;
        }
    }

    if let Some(value) = object.get(KEY_SESSION_STICKY) {
        if !value.is_null() {
            // 只认布尔：与 debug / cors 等单开关的接口口径一致（字符串 "true"
            // 是写坏了，不当成开启）
            let Some(flag) = value.as_bool() else {
                return errors::management_error(
                    400,
                    format!("{KEY_SESSION_STICKY} 必须是布尔值（收到: {value}）"),
                );
            };
            patch.sticky = Some(flag);
            updated = true;
        }
    }

    if updated && !config::set_schedule(patch) {
        // 写盘失败：内存快照已更新（本次运行仍生效），但重启后会回到旧值 ——
        // 必须让用户知道，否则「改了设置重启又变回去」会被当成玄学问题
        logging::log("[Config]", "⚠️  调度设置写入失败，本次运行内仍生效");
    }

    let settings = config::schedule_settings();
    if updated {
        logging::log(
            "[Config]",
            &format!(
                "账号调度已更新: 模式 {} / 会话粘性 {}",
                settings.mode.as_str(),
                if settings.sticky { "开" } else { "关" },
            ),
        );
    }
    ok_json(schedule_json(settings))
}

/// 两项的响应体（GET 与 PUT 共用，键名用常量标识符而不是手写字符串 ——
/// 与 `queue_api::queue_json` 同一手法，键名对前端是契约）。
fn schedule_json(settings: ScheduleSettings) -> Value {
    json!({
        KEY_SCHEDULE_MODE: settings.mode.as_str(),
        KEY_SESSION_STICKY: settings.sticky,
    })
}
