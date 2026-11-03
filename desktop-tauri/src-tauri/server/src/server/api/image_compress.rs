//! 会话图片压缩设置：`GET/PUT /api/image-compress`。
//!
//! ── 这条设置管什么 ──────────────────────────────────────────
//! 转发前把对话请求体里**内嵌的 base64 大图**缩到长边上限并重编码成 JPEG，
//! 既缩小请求体、也降低按像素折算的上游 token 计费；同一张原图在多轮里重复
//! 出现时复用缓存结果（见 [`crate::server::core::upstream::image_compress`]）。
//!
//!   - `imageCompressEnabled`：总开关（默认**开**，见 `config::KEY_IMAGE_COMPRESS_ENABLED`）；
//!   - `imageCompressMaxEdge`：长边像素上限（默认 2048，合法 256~8192）；
//!   - `imageCompressQuality`：JPEG 质量（默认 80，合法 40~95）；
//!   - `imageCompressThresholdKb`：触发阈值 KB（默认 512，合法 0~10240）。
//!
//! ── 只作用在对话场景 ────────────────────────────────────────
//! 实施点在 `core::upstream::payload::send_body`：生图 / 图片编辑走独立分支、
//! 在该函数开头就返回，天然不经过压缩。所以这条设置**不会**影响生图。
//!
//! ── 形状与 /api/auto-continue 同一族 ────────────────────────
//! 一个布尔开关 + 若干有界整数、GET 读 / PUT 写（允许部分字段）、响应体就是
//! 新状态、挂 protected（改的是发给上游的字节）。保存后对下一个请求立即生效
//! （转发层逐请求读内存快照），不重启进程。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Map, Value};

use crate::server::config::parse::int_in_range;
use crate::server::config::{
    self, ImageCompressPatch, ImageCompressSettings, IMAGE_COMPRESS_MAX_MAX_EDGE,
    IMAGE_COMPRESS_MAX_QUALITY, IMAGE_COMPRESS_MAX_THRESHOLD_KB, IMAGE_COMPRESS_MIN_MAX_EDGE,
    IMAGE_COMPRESS_MIN_QUALITY, IMAGE_COMPRESS_MIN_THRESHOLD_KB, KEY_IMAGE_COMPRESS_ENABLED,
    KEY_IMAGE_COMPRESS_MAX_EDGE, KEY_IMAGE_COMPRESS_QUALITY, KEY_IMAGE_COMPRESS_THRESHOLD_KB,
};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// GET /api/image-compress —— 当前开关与三项参数
pub async fn get_image_compress(State(_state): State<ServerState>) -> Response {
    ok_json(state_json(config::image_compress_settings()))
}

/// PUT /api/image-compress —— body `{imageCompressEnabled?, imageCompressMaxEdge?, …}`
///
/// 四项各自独立（缺省项不动），全部校验通过才落盘（与 `put_auto_continue` /
/// `put_retry` 同一顺序），避免出现「开关开了、参数没改」的半套设置。写配置后
/// 下一个请求立即生效。
pub async fn put_image_compress(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let patch = match patch_from(object) {
        Ok(patch) => patch,
        Err(message) => return errors::management_error(400, message),
    };
    if patch == ImageCompressPatch::default() {
        // 一项都没给：不写盘、不记日志，直接把当前状态原样回给调用方
        return ok_json(state_json(config::image_compress_settings()));
    }
    if !config::set_image_compress(patch) {
        logging::log("[Config]", "⚠️  图片压缩设置写入失败，本次运行内仍生效");
    }
    logging::log("[Config]", &change_note(&patch));
    ok_json(state_json(config::image_compress_settings()))
}

/// 请求体 → 部分更新入参（`Err` 的文本就是 400 的文案）。
///
/// 抽成纯函数以便单测（与 `api::auto_continue::patch_from` 同一手法）：
/// 判据要能被钉住，而 handler 需要的 `ServerState` 驱动不了。
///
/// `null` 一律当类型不符（本 PUT 没有「删掉这一项」的语义，缺项即「不动」）。
fn patch_from(object: &Map<String, Value>) -> Result<ImageCompressPatch, String> {
    let mut patch = ImageCompressPatch::default();
    if let Some(value) = object.get(KEY_IMAGE_COMPRESS_ENABLED) {
        let Some(enabled) = value.as_bool() else {
            return Err(format!("{KEY_IMAGE_COMPRESS_ENABLED} 必须是 true 或 false"));
        };
        patch.enabled = Some(enabled);
    }
    if let Some(value) = object.get(KEY_IMAGE_COMPRESS_MAX_EDGE) {
        let Some(number) = int_in_range(value, IMAGE_COMPRESS_MIN_MAX_EDGE, IMAGE_COMPRESS_MAX_MAX_EDGE)
        else {
            return Err(format!(
                "{KEY_IMAGE_COMPRESS_MAX_EDGE} 必须是 {IMAGE_COMPRESS_MIN_MAX_EDGE}~{IMAGE_COMPRESS_MAX_MAX_EDGE} 的整数"
            ));
        };
        patch.max_edge = Some(number);
    }
    if let Some(value) = object.get(KEY_IMAGE_COMPRESS_QUALITY) {
        let Some(number) = int_in_range(value, IMAGE_COMPRESS_MIN_QUALITY, IMAGE_COMPRESS_MAX_QUALITY)
        else {
            return Err(format!(
                "{KEY_IMAGE_COMPRESS_QUALITY} 必须是 {IMAGE_COMPRESS_MIN_QUALITY}~{IMAGE_COMPRESS_MAX_QUALITY} 的整数"
            ));
        };
        patch.quality = Some(number);
    }
    if let Some(value) = object.get(KEY_IMAGE_COMPRESS_THRESHOLD_KB) {
        let Some(number) =
            int_in_range(value, IMAGE_COMPRESS_MIN_THRESHOLD_KB, IMAGE_COMPRESS_MAX_THRESHOLD_KB)
        else {
            return Err(format!(
                "{KEY_IMAGE_COMPRESS_THRESHOLD_KB} 必须是 {IMAGE_COMPRESS_MIN_THRESHOLD_KB}~{IMAGE_COMPRESS_MAX_THRESHOLD_KB} 的整数"
            ));
        };
        patch.threshold_kb = Some(number);
    }
    Ok(patch)
}

/// 这次改动的一句话记录（进事件日志）。只写**动过的那几项**，便于事后追问
/// 「谁把阈值从 512 改成 1024 的」。
fn change_note(patch: &ImageCompressPatch) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(enabled) = patch.enabled {
        parts.push(if enabled {
            "会话图片压缩已开启（转发前压缩内嵌 base64 大图）".to_string()
        } else {
            "会话图片压缩已关闭：图片原样转发（与接入前逐字节一致）".to_string()
        });
    }
    if let Some(max_edge) = patch.max_edge {
        parts.push(format!("长边上限设为 {max_edge}px"));
    }
    if let Some(quality) = patch.quality {
        parts.push(format!("重编码质量设为 {quality}"));
    }
    if let Some(threshold_kb) = patch.threshold_kb {
        parts.push(format!("触发阈值设为 {threshold_kb}KB"));
    }
    if parts.is_empty() {
        "会话图片压缩设置未变".to_string()
    } else {
        parts.join(" / ")
    }
}

/// 四项的响应体（GET 与 PUT 共用；键名对前端是契约）
fn state_json(settings: ImageCompressSettings) -> Value {
    json!({
        KEY_IMAGE_COMPRESS_ENABLED: settings.enabled,
        KEY_IMAGE_COMPRESS_MAX_EDGE: settings.max_edge,
        KEY_IMAGE_COMPRESS_QUALITY: settings.quality,
        KEY_IMAGE_COMPRESS_THRESHOLD_KB: settings.threshold_kb,
    })
}
