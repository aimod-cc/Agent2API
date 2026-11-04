//! Images 响应聚合：不经 Chat 翻译器，保留 OpenAI data 或 Gemini inlineData。

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Value};

use crate::server::core::protocol::SseLineBuffer;
use crate::server::core::providers::images::ImageResponseKind;
use crate::server::errors::GatewayError;

use super::usage::RequestTelemetry;

pub(super) struct ImageFailure {
    pub status: u16,
    pub body: Value,
}

impl ImageFailure {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 502,
            body: json!({ "message": message.into() }),
        }
    }
}

pub(super) enum ImageResponse {
    Live(reqwest::Response),
    Complete(Value),
}

impl ImageResponse {
    pub fn status(&self) -> reqwest::StatusCode {
        match self {
            Self::Live(response) => response.status(),
            Self::Complete(_) => reqwest::StatusCode::OK,
        }
    }
}

pub(super) async fn collect(
    response: reqwest::Response,
    kind: ImageResponseKind,
    transport: &super::request::TransportRequest,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Value, ImageFailure> {
    let budget = Duration::from_millis(crate::server::config::timeout_settings().body_ms());
    let reading = collect_body(response, kind, transport, telemetry);
    let timed = async {
        tokio::time::timeout(budget, reading)
            .await
            .unwrap_or_else(|_| {
                Err(ImageFailure::invalid(format!(
                    "生图响应超时（{}秒），上游可能已消耗额度",
                    budget.as_secs()
                )))
            })
    };
    if let Some(token) = telemetry.cancel_token() {
        tokio::select! {
            result = timed => result,
            _ = token.cancelled() => Err(ImageFailure {
                status: super::cancellation::MANUAL_TERMINATED_STATUS as u16,
                body: json!({ "message": super::cancellation::MANUAL_TERMINATED }),
            }),
        }
    } else {
        timed.await
    }
}

async fn collect_body(
    response: reqwest::Response,
    kind: ImageResponseKind,
    transport: &super::request::TransportRequest,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Value, ImageFailure> {
    const MAX_RESPONSE_BYTES: usize = 96 * 1024 * 1024;
    // 豆包工作：出图在对话流里、且流里给的是带水印预览，要再换一次原图 ——
    // 两步都归 `doubao_body`（那里有为什么必须放在宿主侧的说明）。
    if matches!(kind, ImageResponseKind::Doubao) {
        return doubao_body(response, transport, telemetry).await;
    }
    let status = response.status().as_u16();
    let sse = matches!(kind, ImageResponseKind::Gemini)
        && response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
    let capture = telemetry.capture();
    if let Some(capture) = capture.as_deref() {
        capture.attach_response(status, response.headers());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    let mut lines = SseLineBuffer::new();
    let mut total = 0usize;
    let mut images = Vec::new();
    let mut usage = None;
    let mut finished = false;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| ImageFailure::invalid("上游生图响应传输中断，上游可能已消耗额度"))?;
        total = total.saturating_add(chunk.len());
        if total > MAX_RESPONSE_BYTES {
            return Err(ImageFailure::invalid("上游生图响应超过 96 MiB 上限"));
        }
        if !chunk.is_empty() {
            telemetry.note_first_frame();
        }
        if let Some(capture) = capture.as_deref() {
            capture.push(&chunk);
        }
        if sse {
            for line in lines.push(&chunk) {
                let Some(line) = line else {
                    finished = true;
                    continue;
                };
                let frame: Value = serde_json::from_str(&line)
                    .map_err(|_| ImageFailure::invalid("上游生图 SSE 含不完整的 JSON 帧"))?;
                gemini_frame(&frame, &mut images, &mut usage, &mut finished)?;
            }
        } else {
            bytes.extend_from_slice(&chunk);
        }
    }
    if sse {
        for line in lines.finish() {
            let Some(line) = line else {
                finished = true;
                continue;
            };
            let frame: Value = serde_json::from_str(&line)
                .map_err(|_| ImageFailure::invalid("上游生图 SSE 被截断"))?;
            gemini_frame(&frame, &mut images, &mut usage, &mut finished)?;
        }
        if !finished {
            return Err(ImageFailure::invalid(
                "上游生图流没有正常收尾，未将可能截断的图片当作成功",
            ));
        }
    } else {
        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ImageFailure::invalid("上游生图响应不是有效 JSON"))?;
        if matches!(kind, ImageResponseKind::ModelscopeAsync) {
            return super::image_tasks::collect(&payload, transport, telemetry).await;
        }
        if matches!(kind, ImageResponseKind::Workbuddy) {
            return workbuddy_body(&payload, status, telemetry);
        }
        if matches!(kind, ImageResponseKind::Openai) {
            if payload.get("error").is_some_and(|error| !error.is_null()) {
                return Err(failure_from(&payload, status));
            }
            let data = payload.get("data").and_then(Value::as_array);
            if data.is_some_and(|data| !data.is_empty()) {
                if let Some(usage) = payload.get("usage") {
                    telemetry.report_usage(usage);
                }
                // 有的家把本次消耗的积分 / 点数**直接放在响应顶层**（不在 usage 里）：
                // Loomy 生图的 `points_consumed` 就是实测形态。按 credit 单键补报一次，
                // 让它进报表的积分列而不是被当成 token。
                if let Some(credit) = payload
                    .as_object()
                    .and_then(|object| super::usage::credit_from_object(object))
                {
                    telemetry.report_usage(&json!({ "credit": credit }));
                }
                return Ok(payload);
            }
            return Err(failure_from(&payload, status));
        }
        gemini_frame(&payload, &mut images, &mut usage, &mut finished)?;
    }
    if images.is_empty() {
        return Err(ImageFailure::invalid(
            "上游没有返回生成图片（可能被内容策略拦截），未将文字回复当作生图成功",
        ));
    }
    let mut result = json!({ "created": crate::server::logging::now_ms() / 1000, "data": images });
    if let Some(usage) = usage {
        telemetry.report_usage(&usage);
        result["usage"] = usage;
    }
    Ok(result)
}

/// 豆包工作的生图响应：从对话流里取图片，优先取**无水印原图**。
///
/// ── 为什么放在宿主侧 ────────────────────────────────────────
/// 上游没有独立的生图接口：出图就是**在对话里让 agent 干活**，结果作为
/// `gen_image_block`（`block_type = 10010`）挂在同一条 SSE 流里。而这一路是
/// `ImageResponseKind::Doubao` 才有的解析（普通的 OpenAI / Gemini 聚合在这里
/// 全都不适用），与 `ModelscopeAsync`（提交后轮询）是同一取舍：插件的
/// `ImagePlan` 只表达"一次请求"，读这种流要按豆包的块结构来。
///
/// ── 无水印原图从哪来 ────────────────────────────────────────
/// 实测（2026-11-03）流里有两种块：`10010` 给的是**带水印的压缩预览**
/// （`~tplv-…-downsize_watermark…`，134 KB、左上角带「AI 生成」角标），
/// `10084`（`rich_media_layout_block`）另外给了**无水印原图**
/// （`~tplv-…-image_raw.png`）—— 它与官方
/// `/creativity/resource/get_without_watermark` 的 `cgen_wm0` **字节完全相同**
/// （4624288 字节、同一 md5），而且晚一步到。所以默认一次请求就够；
/// [`doubao_without_watermark`] 只在流里没给原图时补一次（旧版本/画布块缺失）。
///
/// ── 换原图失败不致命 ────────────────────────────────────────
/// 拿不到原图就退回预览：有图总比报错好，只记一行日志。
async fn doubao_body(
    response: reqwest::Response,
    transport: &super::request::TransportRequest,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Value, ImageFailure> {
    /// 生图这一轮里 agent 会顺带说不少话，给个上限（正常几十 KB）。
    const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
    let status = response.status().as_u16();
    let capture = telemetry.capture();
    if let Some(capture) = capture.as_deref() {
        capture.attach_response(status, response.headers());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| ImageFailure::invalid("上游生图响应传输中断，上游可能已消耗额度"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_STREAM_BYTES {
            return Err(ImageFailure::invalid("上游生图响应超过 8 MiB 上限"));
        }
        if let Some(capture) = capture.as_deref() {
            capture.push(&chunk);
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&bytes);
    let images = doubao_images(&text);
    if images.is_empty() {
        // 上游可能是直接报错（风控 / 参数）：把它的错误摘出来，
        // 别报成"没有生成图片"。两种形态都看 —— 流内 `data:` 帧，以及
        // 整段就是一个 JSON 对象（有些失败分支不走 SSE，实测风控那次就是）。
        let failure = doubao_stream_failure(&text).or_else(|| {
            serde_json::from_str::<Value>(text.trim())
                .ok()
                .and_then(|value| doubao_value_failure(&value))
        });
        if let Some(failure) = failure {
            return Err(failure);
        }
        return Err(ImageFailure::invalid(
            "豆包没有返回图片（可能被内容策略拦截），未把文字回复当作生图成功",
        ));
    }
    // 流里通常已经带了无水印原图（`10084` 的 `image_ori_raw`）；只有缺了才去调
    // 官方去水印接口 —— 那是一次额外请求，能省就省（见 `DoubaoImage::raw`）。
    let missing: Vec<DoubaoImage> = images
        .iter()
        .filter(|image| image.raw.is_none())
        .cloned()
        .collect();
    let clean = if missing.is_empty() {
        Vec::new()
    } else {
        doubao_without_watermark(transport, &missing).await
    };
    let data = images
        .iter()
        .map(|image| {
            let url = image
                .raw
                .clone()
                .or_else(|| {
                    clean
                        .iter()
                        .find(|(key, _)| key == &image.key)
                        .map(|(_, url)| url.clone())
                })
                .unwrap_or_else(|| image.preview.clone());
            json!({ "url": url })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "created": crate::server::logging::now_ms() / 1000,
        "data": data,
    }))
}

/// 流里收到的一张图（两种块都会报同一张，按对象键合并）。
#[derive(Clone)]
struct DoubaoImage {
    /// 对象键（`tos-cn-i-…/rc_gen_image/x.jpeg`），换原图那个接口收的就是它。
    key: String,
    /// 无水印原图（`10084` 的 `image_ori_raw` / `~tplv-…-image_raw.png`）。
    ///
    /// 实测（2026-11-03）同一张图：流里这份 `image_raw` 与官方
    /// `/creativity/resource/get_without_watermark` 返回的 `cgen_wm0` **字节完全
    /// 相同**（4624288 字节、同一 md5）。所以正常情况下一次请求就够，**不必**
    /// 再发第二次；`doubao_without_watermark` 只服务"流里没给"的兜底情形。
    raw: Option<String>,
    /// 带水印的预览（`10010` 的 `uri` / `10084` 的 `image_ori`），最后的兜底。
    preview: String,
}

/// 按对象键把一张图并进结果（先到的补预览、后到的补原图，两种顺序都成立）。
fn doubao_merge(
    found: &mut Vec<DoubaoImage>,
    key: String,
    raw: Option<String>,
    preview: Option<String>,
) {
    if let Some(known) = found.iter_mut().find(|known| known.key == key) {
        if known.raw.is_none() {
            known.raw = raw;
        }
        if known.preview.is_empty() {
            known.preview = preview.unwrap_or_default();
        }
        return;
    }
    found.push(DoubaoImage {
        key,
        raw,
        preview: preview.unwrap_or_default(),
    });
}

/// 从豆包的 SSE 文本里取图片（按对象键去重、保持先后）。
///
/// ── 两种块都要看 ────────────────────────────────────────────
/// `10010`（`gen_image_block`，交付结果）给的是**带水印的预览**；
/// `10084`（`rich_media_layout_block`，画布渲染）给的是**无水印原图**
/// （`image_ori_raw`）与对象键。只认前者就只能拿到带水印那张，只认后者则在
/// 画布块缺失时就什么都没有 —— 所以两边都收，按对象键合并（见 `DoubaoImage`）。
fn doubao_images(text: &str) -> Vec<DoubaoImage> {
    let mut found: Vec<DoubaoImage> = Vec::new();
    for data in doubao_data_frames(text) {
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let mut blocks: Vec<&Value> = Vec::new();
        for patch in value
            .get("patch_op")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if patch.get("patch_object").and_then(Value::as_u64) != Some(1) {
                continue;
            }
            blocks.extend(
                patch
                    .pointer("/patch_value/content_block")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            );
        }
        for holder in [
            &value["content"]["content_block"],
            &value["message"]["content_block"],
        ] {
            blocks.extend(holder.as_array().into_iter().flatten());
        }
        for block in blocks {
            match block.get("block_type").and_then(Value::as_u64) {
                Some(10010) => {
                    for image in block
                        .pointer("/content/gen_image_block/images")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let Some(url) = ["uri", "thumb_url"].iter().find_map(|key| {
                            image[*key]
                                .as_str()
                                .filter(|value| !value.trim().is_empty())
                        }) else {
                            continue;
                        };
                        let Some(key) = doubao_object_key(url) else {
                            continue;
                        };
                        doubao_merge(&mut found, key, None, Some(url.to_string()));
                    }
                }
                Some(10084) => {
                    for media in block
                        .pointer("/content/rich_media_layout_block/media")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let image = &media["creation"]["image"];
                        let key = image["key"]
                            .as_str()
                            .filter(|value| !value.trim().is_empty())
                            .map(str::to_string)
                            .or_else(|| {
                                ["image_ori", "image_preview", "image_thumb"]
                                    .iter()
                                    .find_map(|field| {
                                        doubao_object_key(image[*field]["url"].as_str()?)
                                    })
                            });
                        let Some(key) = key else { continue };
                        let raw = image["image_ori_raw"]["url"]
                            .as_str()
                            .filter(|value| !value.trim().is_empty())
                            .map(str::to_string);
                        let preview = ["image_ori", "image_preview", "image_thumb"]
                            .iter()
                            .find_map(|field| {
                                image[*field]["url"]
                                    .as_str()
                                    .filter(|value| !value.trim().is_empty())
                            })
                            .map(str::to_string);
                        doubao_merge(&mut found, key, raw, preview);
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// 图片地址 → 对象键。
///
/// 地址形如 `https://host/tos-cn-i-…/rc_gen_image/x.jpeg~tplv-…?…`，对象键是
/// `~` 之前、去掉协议与 host 的那一段（`tos-cn-i-…/rc_gen_image/x.jpeg`）——
/// 换原图的那个接口收的就是它。
fn doubao_object_key(url: &str) -> Option<String> {
    let without_query = url.split('?').next().unwrap_or(url);
    let path = without_query.split('~').next().unwrap_or(without_query);
    let key = path
        .strip_prefix("https://")
        .or_else(|| path.strip_prefix("http://"))
        .map(|rest| rest.split_once('/').map(|(_, path)| path).unwrap_or(""))
        .unwrap_or(path);
    (!key.is_empty()).then(|| key.to_string())
}

/// 从 SSE 文本里取 `data:` 载荷（不关心 `event:` 名字 —— 这里只看内容）。
fn doubao_data_frames(text: &str) -> Vec<String> {
    let mut frames = Vec::new();
    for block in text.split("\n\n") {
        let mut data: Vec<&str> = Vec::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data.push(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        if !data.is_empty() {
            frames.push(data.join("\n"));
        }
    }
    frames
}

/// 流里没有图片时，多半是上游直接报了错：逐帧取错误，让用户看到原因。
fn doubao_stream_failure(text: &str) -> Option<ImageFailure> {
    doubao_data_frames(text)
        .iter()
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find_map(|value| doubao_value_failure(&value))
}

/// 一个豆包失败体 → 归一化失败：把 code/message 摘出来，风控那条 `710022004`
/// 单独给一句能照做的文案（与插件侧同一口径）。
///
/// 只认豆包错误信封自己的键（`error_code` / `error_msg` / `error_message` /
/// `detail`，实测形如 `{"error_code":710022004,"error_msg":"rate limited","extra":…}`）：
/// 不去认通用的 `message` / `msg` —— 正常流里也有 `message` 这个**对象**字段，
/// 认宽了会把一次正常但没出图的答复报成上游故障。
fn doubao_value_failure(value: &Value) -> Option<ImageFailure> {
    let code = value.get("error_code").and_then(Value::as_u64);
    let message = ["error_msg", "error_message", "detail"]
        .iter()
        .find_map(|key| value[*key].as_str().filter(|text| !text.trim().is_empty()));
    if code.is_none() && message.is_none() {
        return None;
    }
    let message = message.unwrap_or("上游返回了无法解析的错误");
    if code == Some(710022004) {
        return Some(ImageFailure {
            status: 429,
            body: json!({
                "message": "豆包工作生图触发了上游人机验证：请在该账号的豆包网页端发一条消息完成验证后重试；也可以等待约 30 分钟或更换出口网络再试",
                "error_code": 710022004,
            }),
        });
    }
    Some(ImageFailure {
        status: 502,
        body: json!({ "message": format!("豆包工作生图失败：{message}"), "error_code": code }),
    })
}

/// 拿对象键换无水印原图地址（对象键 → 地址）。
///
/// 失败一律返回空表（调用方退回预览地址）：这一步只是"锦上添花"，不该因为它
/// 把已经生成好的图片整条丢掉。
async fn doubao_without_watermark(
    transport: &super::request::TransportRequest,
    images: &[DoubaoImage],
) -> Vec<(String, String)> {
    let Ok(base) = url::Url::parse(&transport.url) else {
        return Vec::new();
    };
    let Ok(mut target) = base.join("/creativity/resource/get_without_watermark") else {
        return Vec::new();
    };
    // 查询串要照搬：那几个 client 参数（device_id / pc_version / fp …）是接口
    // 认这个账户与设备的依据，缺了就会被当成未受信客户端。
    target.set_query(base.query());
    let keys: Vec<&str> = images.iter().map(|image| image.key.as_str()).collect();
    let client = crate::server::core::egress::client_without_redirects(transport.proxy.as_ref());
    let mut request = client
        .post(target)
        .timeout(Duration::from_secs(60))
        .json(&json!({ "uri": keys }));
    for (name, value) in &transport.headers {
        // `content-length` / `content-type` 交给 reqwest 按 `.json()` 自己写：
        // 照搬会多出第二条同名字段（父请求的长度还是 SSE 那份）。
        if name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("content-type")
        {
            continue;
        }
        request = request.header(name, value);
    }
    let response = match request
        .header("Accept", "application/json, text/plain, */*")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            crate::server::logging::verbose(
                "[Upstream]",
                &format!("豆包换无水印原图失败，改用预览地址：{error}"),
            );
            return Vec::new();
        }
    };
    let status = response.status().as_u16();
    let Ok(value) = response.json::<Value>().await else {
        crate::server::logging::verbose(
            "[Upstream]",
            "豆包换无水印原图的响应不是 JSON，改用预览地址",
        );
        return Vec::new();
    };
    if !(200..300).contains(&status) || value.get("code").and_then(Value::as_i64).unwrap_or(0) != 0
    {
        crate::server::logging::verbose(
            "[Upstream]",
            &format!("豆包换无水印原图被拒（HTTP {status}），改用预览地址"),
        );
        return Vec::new();
    }
    let data = value.get("data").unwrap_or(&Value::Null);
    if data.get("without_watermark").and_then(Value::as_bool) != Some(true) {
        crate::server::logging::verbose("[Upstream]", "该账号没有无水印权益，改用预览地址");
        return Vec::new();
    }
    let mut found = Vec::new();
    for key in ["download_image", "preview_image"] {
        for (uri, entry) in data
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let Some(url) = entry
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
            else {
                continue;
            };
            found.push((uri.clone(), url.to_string()));
        }
        if !found.is_empty() {
            break;
        }
    }
    found
}

fn gemini_frame(
    frame: &Value,
    images: &mut Vec<Value>,
    usage: &mut Option<Value>,
    finished: &mut bool,
) -> Result<(), ImageFailure> {
    let response = frame.get("response").unwrap_or(frame);
    if response.get("error").is_some_and(|error| !error.is_null()) {
        return Err(failure_from(response, 200));
    }
    if frame.get("error").is_some_and(|error| !error.is_null()) {
        return Err(failure_from(frame, 200));
    }
    if let Some(metadata) = response
        .get("usageMetadata")
        .or_else(|| response.get("usage_metadata"))
    {
        let input = metadata
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = metadata
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let thought = metadata
            .get("thoughtsTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        *usage = Some(json!({
            "input_tokens": input,
            "output_tokens": output.saturating_add(thought),
            "total_tokens": metadata.get("totalTokenCount").and_then(Value::as_u64)
                .unwrap_or(input.saturating_add(output).saturating_add(thought)),
        }));
    }
    let Some(candidates) = response.get("candidates").and_then(Value::as_array) else {
        if response.pointer("/promptFeedback/blockReason").is_some() {
            return Err(ImageFailure {
                status: 400,
                body: json!({ "code": "content_policy_violation", "message": "上游内容策略拦截了生图提示词" }),
            });
        }
        return Ok(());
    };
    for candidate in candidates {
        if candidate.get("finishReason").and_then(Value::as_str) == Some("STOP") {
            *finished = true;
        }
        if let Some(parts) = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
        {
            for part in parts {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let Some(inline) = part.get("inlineData").or_else(|| part.get("inline_data"))
                else {
                    continue;
                };
                let Some(data) = inline
                    .get("data")
                    .and_then(Value::as_str)
                    .filter(|data| !data.is_empty())
                else {
                    continue;
                };
                let mime = inline
                    .get("mimeType")
                    .or_else(|| inline.get("mime_type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !mime.starts_with("image/") {
                    continue;
                }
                if !images
                    .iter()
                    .any(|image| image.get("b64_json").and_then(Value::as_str) == Some(data))
                {
                    images.push(json!({ "b64_json": data, "mime_type": mime }));
                }
            }
        }
        if candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .is_some_and(|reason| {
                matches!(reason, "SAFETY" | "IMAGE_SAFETY" | "PROHIBITED_CONTENT")
            })
        {
            return Err(ImageFailure {
                status: 400,
                body: json!({ "code": "content_policy_violation", "message": "上游内容策略拦截了图片输出" }),
            });
        }
    }
    Ok(())
}

/// WorkBuddy 的图片响应多一层 `{code, msg, request_id, data:{data:[…], usage:{credit}}}`
/// 信封：`code != 0` 是业务失败（HTTP 可能仍是 200），图片数组在 `data.data[]`
/// 里（条目与 OpenAI 同形：`b64_json` 或 `url`），`data.usage.credit` 是本单消耗
/// 的积分 —— 不是 token，所以按 `credit` 单键上报，让它进报表的积分列而不是
/// 伪装成 token 用量。
fn workbuddy_body(
    payload: &Value,
    http_status: u16,
    telemetry: &Arc<RequestTelemetry>,
) -> Result<Value, ImageFailure> {
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 0 {
        return Err(failure_from(payload, http_status));
    }
    let envelope = payload.get("data").filter(|data| data.is_object());
    let data = envelope
        .and_then(|data| data.get("data"))
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| failure_from(payload, http_status))?;
    if let Some(credit) = envelope.and_then(|data| data.pointer("/usage/credit")) {
        telemetry.report_usage(&json!({ "credit": credit.clone() }));
    }
    Ok(json!({
        "created": crate::server::logging::now_ms() / 1000,
        "data": data,
    }))
}

fn failure_from(payload: &Value, http_status: u16) -> ImageFailure {
    let error = payload.get("error").unwrap_or(payload);
    let code = error.get("code").cloned().unwrap_or(Value::Null);
    let message = error
        .get("message")
        .or_else(|| error.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("上游没有返回有效的图片数据");
    let upstream_status = error.get("status").and_then(Value::as_str).unwrap_or("");
    let status = if http_status >= 400 {
        http_status
    } else {
        match upstream_status {
            "UNAUTHENTICATED" => 401,
            "PERMISSION_DENIED" => 403,
            "RESOURCE_EXHAUSTED" => 429,
            "INVALID_ARGUMENT" => 400,
            _ => code
                .as_u64()
                .filter(|code| (400..600).contains(code))
                .map(|code| code as u16)
                .unwrap_or(502),
        }
    };
    ImageFailure {
        status,
        body: json!({ "code": code, "message": format!("{upstream_status} {message}").trim() }),
    }
}

pub(super) fn output_error(message: impl Into<String>) -> GatewayError {
    GatewayError::with_status(502, message).with_code("image_generation_error")
}
