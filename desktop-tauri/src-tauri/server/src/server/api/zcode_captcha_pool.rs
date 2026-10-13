//! GET/PUT /api/zcode/captcha-pool —— 验证码令牌池的**补货节奏**设置。
//!
//! ── 与 `/api/zcode/captcha` 的分工（名字像，职责不同）─────────
//! 那边是池子本身：GET 是铸造器每轮轮询的**库存概况**，POST 是铸造好的令牌
//! 入池（见 `zcode_captcha.rs` 的模块头）；这边是**节奏设置**——库存目标、
//! 铸造器轮询间隔、单轮铸造上限（见 `config::CaptchaPoolSettings`）。
//! GET stats 的响应会把这三项一并带给铸造器（`target` / `pollMs` /
//! `maxPerRound`），所以保存后下一轮轮询就按新值跑，**不用重启**。
//!
//! ── 为什么要做成设置 ────────────────────────────────────────
//! 三个值原本硬编码（3 / 2500 / 1），是「一个够用又不浪费」的保守档：令牌
//! 一次性、120 秒过期，阿里云对铸造频率有风控。但流量大时铸造吞吐跟不上
//! 消耗，池子长期贴地、请求连续 503 —— 用户需要能按自己的请求节奏放宽
//! （调大目标 / 单轮上限、调快轮询），也要能在风控收紧时收窄。代价写进
//! 设置页的说明里：调大烧的是阿里云风控配额，过期令牌一律作废。
//!
//! 与 /api/queue 同一模式：独立端点、允许部分字段、返回**生效后**的全量值、
//! 无副作用（不重启进程）。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::config::{
    self, CaptchaPoolPatch, CaptchaPoolSettings, KEY_CAPTCHA_POOL_MAX_PER_ROUND,
    KEY_CAPTCHA_POOL_POLL_MS, KEY_CAPTCHA_POOL_TARGET, CAPTCHA_POOL_MAX_MAX_PER_ROUND,
    CAPTCHA_POOL_MAX_POLL_MS, CAPTCHA_POOL_MAX_TARGET, CAPTCHA_POOL_MIN_MAX_PER_ROUND,
    CAPTCHA_POOL_MIN_POLL_MS, CAPTCHA_POOL_MIN_TARGET,
};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/zcode/captcha-pool
pub async fn get_captcha_pool(State(_state): State<ServerState>) -> Response {
    ok_json(captcha_pool_json(config::captcha_pool_settings()))
}

/// PUT /api/zcode/captcha-pool —— body `{zcodeCaptchaPoolTarget?, zcodeCaptchaPoolPollMs?, zcodeCaptchaPoolMaxPerRound?}`
///
/// 允许部分字段（未出现的项保持原值，null 同义）。校验通过后：写配置库
/// → 返回**生效后**的值（前端直接用响应刷新界面，不必再 GET 一次）。
pub async fn put_captcha_pool(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };

    // 校验与写盘分两步（与 put_queue 同一顺序）：三项里有一项非法时整体
    // 不落盘，避免出现「目标改了、节奏没改」的半套设置
    let mut patch = CaptchaPoolPatch::default();
    let mut updated = false;
    let targets = [
        (
            KEY_CAPTCHA_POOL_TARGET,
            object.get(KEY_CAPTCHA_POOL_TARGET),
            CAPTCHA_POOL_MIN_TARGET,
            CAPTCHA_POOL_MAX_TARGET,
            "库存目标",
            &mut patch.target,
        ),
        (
            KEY_CAPTCHA_POOL_POLL_MS,
            object.get(KEY_CAPTCHA_POOL_POLL_MS),
            CAPTCHA_POOL_MIN_POLL_MS,
            CAPTCHA_POOL_MAX_POLL_MS,
            "轮询间隔",
            &mut patch.poll_ms,
        ),
        (
            KEY_CAPTCHA_POOL_MAX_PER_ROUND,
            object.get(KEY_CAPTCHA_POOL_MAX_PER_ROUND),
            CAPTCHA_POOL_MIN_MAX_PER_ROUND,
            CAPTCHA_POOL_MAX_MAX_PER_ROUND,
            "单轮铸造上限",
            &mut patch.max_per_round,
        ),
    ];
    for (key, value, min, max, label, slot) in targets {
        let Some(value) = value else {
            continue;
        };
        // null 视为「这一项不改」：前端整份回传时未填项常见为 null
        if value.is_null() {
            continue;
        }
        match parse_bounded_int(key, value, min, max, label) {
            Ok(number) => {
                *slot = Some(number);
                updated = true;
            }
            Err(message) => return errors::management_error(400, message),
        }
    }

    if updated && !config::set_captcha_pool(patch) {
        // 写盘失败：内存快照已更新（本次运行仍生效），但重启后会回到旧值 ——
        // 必须让用户知道，否则「改了设置重启又变回去」会被当成玄学问题
        logging::log("[Config]", "⚠️  验证码令牌池设置写入失败，本次运行内仍生效");
    }

    let settings = config::captcha_pool_settings();
    if updated {
        logging::log(
            "[Config]",
            &format!(
                "验证码令牌池已更新: 目标 {} 个 / 轮询 {}ms / 单轮最多 {} 个",
                settings.target, settings.poll_ms, settings.max_per_round
            ),
        );
    }
    ok_json(captcha_pool_json(settings))
}

/// 三项的响应体（GET 与 PUT 共用，键名用常量标识符而不是手写字符串 ——
/// 与 `queue_api::queue_json` 同一手法，键名对前端是契约）。
fn captcha_pool_json(settings: CaptchaPoolSettings) -> Value {
    json!({
        KEY_CAPTCHA_POOL_TARGET: settings.target,
        KEY_CAPTCHA_POOL_POLL_MS: settings.poll_ms,
        KEY_CAPTCHA_POOL_MAX_PER_ROUND: settings.max_per_round,
    })
}

/// 单个整数的校验：必须是范围内的整数，否则给出可读的 400 文案。
/// 与 `queue_api::parse_bounded_int` 同一口径（只认 JSON 数字，容忍 `2.0`
/// 这种整值浮点；字符串 `"2"` 视为非法）。
fn parse_bounded_int(
    key: &str,
    value: &Value,
    min: i64,
    max: i64,
    label: &str,
) -> Result<i64, String> {
    let number = match value {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|raw| raw.is_finite() && raw.fract() == 0.0)
                .map(|raw| raw as i64)
        }),
        _ => None,
    };
    let Some(number) = number else {
        return Err(format!("{label}（{key}）必须是整数"));
    };
    if !(min..=max).contains(&number) {
        return Err(format!("{label}（{key}）必须在 {min}~{max} 之间"));
    }
    Ok(number)
}
