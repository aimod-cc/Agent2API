//! Loomy 上游的图片格式适配：哪些模型要把 `image_url` 换成 `<image_base64>` 标记。
//!
//! ── 上游按模型认两种图片形态（直连 loomyad 实测，2026-10）────
//! 分界线是**上游目录 `/api/v1/models` 自己声明的 `vision` 位**，不是模型名：
//!   - `vision: true`（`DeepSeek-V4.1-Flash` / `GLM-5.3-Flash` / `Kimi-k2.6` /
//!     `MiniMax-M3` / `qwen3.8-flash` …）：认标准 OpenAI `image_url` data URI，
//!     图片按**视觉 token** 计费、与 base64 长度无关 —— 实测 128×128 同一张图，
//!     348 字符与 65800 字符两次请求 prompt 都恰好是 241 token，且模型说对了
//!     画面的实际颜色顺序。反过来喂 `<image_base64>` 标记时，上游把 base64 当
//!     **纯文本**塞进 prompt（约 2.6 字符 1 token），模型只会空转着解码：同一张
//!     348 字符的小图跑了 148 秒、烧掉 29326 个推理 token 且一个字答案都没给出，
//!     65800 字符那次直接 150 秒被网关 504 掐断。
//!   - `vision: false`（`deepseek-v4-flash-0731` / `qwen-3.8-max` / `spark-x`）：
//!     上游没有视觉通路，`image_url` 里的 base64 同样只会被当文本喂进去（实测
//!     同一张非 RGB 顺序的图，`deepseek-v4-flash-0731` 答成「红 绿 蓝」——
//!     它看不见，答的是 RGB 常识）。标记写法是这一档的兜底（上游把标记转成视觉
//!     token：同一张图 prompt 164 → 7650，模型能答出画面内容）。
//!
//! 官方客户端本身不含这个标记（解包 `app.asar` 里没有任何 `image_base64`
//! 字面量），它只是**上游网关自己的表示**；客户端是照目录的
//! `capabilities.input_modalities` 决定发不发 `image_url`。我们同一口径：
//! 按目录声明放行，宁可不转，不能转错。
//!
//! ── 判据为什么不能用模型名字面量（2026-10-17 修正）────────────
//! 原先用 `contains("deepseek")` 一把抓，把**声明支持图片的**
//! `DeepSeek-V4.1-Flash` 也按只认标记的 0731 处理了：两张 JPEG 被改写成
//! 82.8 万字符的 base64 文本，prompt 从 33.2 万涨到 65.8 万（用户报的
//! 「切到 Loomy 上下文翻倍」），而且这段文本每轮对话都要重发。判据落在
//! **上游自己声明的能力**上才不会误伤同名前缀的视觉模型，见 [`needs_tag_format`]。
//!
//! ── 标记并入相邻文本块（有实测依据的形态）────────────────────
//! 直连验证通过的形态是「一段文本 + 标记写在同一段里」（content 为字符串）；
//! 数组里独立 text 块给上游的形态没有实测依据，所以这里把标记并入前一个
//! text 块（没有前块时并入后块、再没有才独立成块）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 panic=abort：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use super::models;
use crate::server::core::providers::catalog::wire_target_for_provider;
use crate::server::core::providers::{kind_id, ProviderKind};

/// 转发前对图片做的适配：返回 (要发出去的 body, 转换的图片数)。
///
/// 无图请求（绝大多数）只付一次借用扫描就返回 —— 不解析 wire 模型、不查目录。
pub fn adapt_images(body: &Value, requested_model: &str) -> (Value, usize) {
    if !has_convertible_image(body) {
        return (body.clone(), 0);
    }
    if !needs_tag_format(requested_model) {
        return (body.clone(), 0);
    }
    rewrite_to_tag(body)
}

/// 该请求的图片要不要转成 `<image_base64>` 标记（判据与依据见模块头）。
///
/// 看的是**上游目录对「真正要发出去的那个模型」的图片输入声明**：
///   · 目录里有这条记录、且声明了 `supportsImages` → 不转（发标准 image_url）；
///   · 目录里有这条记录、但没有这个能力位 → 转（上游没有视觉通路，标记是唯一
///     验证过的兜底）；
///   · 目录里**查不到**（目录还没刷出来、模型是自定义的 / 上游已下线）→ 回落
///     旧的模型名字面量判据，与改动前逐字一致：冷启动不该静默改变转发形态。
///
/// 先解析 wire 模型（与发送侧 `payload` 用同一个函数，见
/// `providers::catalog::routing`）：客户端请求名 `deepseek-v4.1-flash` 与目录 id
/// `DeepSeek-V4.1-Flash` 并不是同一个串，拿请求名直接查目录会漏。
pub fn needs_tag_format(requested_model: &str) -> bool {
    let wire =
        wire_target_for_provider(requested_model, kind_id(ProviderKind::Loomy), None).model;
    match declared_image_input(&wire) {
        Some(supported) => !supported,
        None => legacy_name_rule(&wire),
    }
}

/// 上游目录对 `wire` 模型声明的图片输入能力。
/// `Some(true|false)` = 目录里有这条记录（能力位缺失按 `false` 读 —— 与
/// `models::map_entry` 只在上游声明时才写这个键是同一口径）；`None` = 查不到。
fn declared_image_input(wire: &str) -> Option<bool> {
    let wire = wire.trim();
    if wire.is_empty() {
        return None;
    }
    models::list()
        .iter()
        .find(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.eq_ignore_ascii_case(wire))
        })
        .map(|item| {
            item.get("supportsImages")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
}

/// 旧判据（模型名里含 `deepseek`）：只在目录查不到这个模型时兜底。
fn legacy_name_rule(model: &str) -> bool {
    model.to_ascii_lowercase().contains("deepseek")
}

/// 把 messages 里的 data URI 图片块转成 `<image_base64>` 标记。返回
/// (改写后的 body, 转换的图片数)；没有可转的图片时返回原 body 的克隆 + 0。
///
/// 先借用扫描一遍判断「有没有可转的图」，有才深拷贝 —— 无图片的 deepseek
/// 请求（绝大多数）不付这次拷贝的成本。
pub fn rewrite_to_tag(body: &Value) -> (Value, usize) {
    if !has_convertible_image(body) {
        return (body.clone(), 0);
    }
    let mut next = body.clone();
    let mut converted = 0usize;
    let Some(messages) = next.get_mut("messages").and_then(Value::as_array_mut) else {
        return (next, 0);
    };
    for message in messages.iter_mut() {
        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        // 倒序一轮：替换/合并会改变下标，从后往前才不会被已处理的位置带偏
        let mut index = content.len();
        while index > 0 {
            index -= 1;
            let Some(tag) = to_tag_text(&content[index]) else {
                continue;
            };
            content.remove(index);
            if let Some(text) = index
                .checked_sub(1)
                .and_then(|previous| text_part_mut(&mut content[previous]))
            {
                // 优先并入**前一个** text 块：与原消息的文本顺序一致，
                // 「这条文本在说这张图」的关系保持最近
                text.push('\n');
                text.push_str(&tag);
            } else if let Some(text) = content.get_mut(index).and_then(text_part_mut) {
                text.insert_str(0, &format!("{tag}\n"));
            } else {
                content.insert(index, json!({ "type": "text", "text": tag }));
            }
            converted += 1;
        }
    }
    (next, converted)
}

/// messages 里有没有至少一个可转的 data URI 图片块（借用扫描，不拷贝）
fn has_convertible_image(body: &Value) -> bool {
    body.get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|content| content.iter().any(|part| to_tag_text(part).is_some()))
            })
        })
}

/// 单个图片块 → `<image_base64>…</image_base64>` 标记文本。
/// 只认 data URI（`data:image/…;base64,…`）；http(s) 图片 URL 不转 ——
/// 标记里放 URL 没有意义，上游认不认又没有实测依据，保持原样最保守。
fn to_tag_text(part: &Value) -> Option<String> {
    let kind = part
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if kind != "image_url" && kind != "input_image" {
        return None;
    }
    // 两种 url 形态都认：`{"image_url":{"url":"data:…"}}`（OpenAI 标准）
    // 与 `{"image_url":"data:…"}`（宽口径客户端的简化写法）
    let url = part
        .get("image_url")
        .and_then(|value| {
            value
                .get("url")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
        })
        .map(str::trim)
        .filter(|url| !url.is_empty())?;
    let base64 = data_uri_base64(url)?;
    Some(format!("<image_base64>{base64}</image_base64>"))
}

/// `data:image/png;base64,AAAA…` → `AAAA…`（非 data URI / 非 base64 → None）
fn data_uri_base64(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.to_ascii_lowercase().contains("base64") {
        return None;
    }
    let payload = payload.trim();
    if payload.is_empty() {
        return None;
    }
    Some(payload)
}

/// text 块的可写文本（不是 `{type:"text", text:"…"}` 形态时 None）
fn text_part_mut(part: &mut Value) -> Option<&mut String> {
    let is_text = part
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
        == Some("text");
    if !is_text {
        return None;
    }
    match part.get_mut("text") {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}
