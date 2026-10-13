//! Loomy 上游的图片格式适配：哪些模型要把 `image_url` 换成 `<image_base64>` 标记。
//!
//! ── 上游按模型认两种图片形态（直连 loomyad 实测，2026-10）────
//!   - `GLM-5.3-Flash`：认标准 OpenAI `image_url` data URI（识别准确）；
//!     喂 `<image_base64>` 标记时回复「无法查看图片，这是一段 Base64 字符串」。
//!   - `deepseek-v4-flash-0731`：只认 `<image_base64>…</image_base64>` 标记
//!     （上游把标记转成视觉 token —— 同一张图 prompt 从 164 → 7650 token，
//!     模型能答出画面内容）；喂 image_url 时模型看到的是会被截断的 base64
//!     文本，回复「图片以编码/截断的形式发过来」，即用户报的「不支持图片」。
//!
//! 官方客户端本身不含这个标记（解包 `app.asar` 里没有任何 `image_base64`
//! 字面量），说明它是**上游网关自己的视觉 token 表示** —— 我们只能按实测
//! 结果按模型适配：deepseek 系转标记，其余保持 image_url。宁可不转，不能
//! 转错：GLM 转了反而看不了图。
//!
//! ── 标记并入相邻文本块（有实测依据的形态）────────────────────
//! 直连验证通过的形态是「一段文本 + 标记写在同一段里」（content 为字符串）；
//! 数组里独立 text 块给上游的形态没有实测依据，所以这里把标记并入前一个
//! text 块（没有前块时并入后块、再没有才独立成块）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 panic=abort：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

/// 该模型是否要把图片转成 `<image_base64>` 标记（判据与依据见模块头）。
///
/// 用 `contains` 而不是精确 id：模型映射的别名（`deepseek-v4.1-flash` →
/// `deepseek-v4-flash-0731`）在改写前拿到的还是客户端请求名，两者都含
/// `deepseek`，宽松匹配对两段链路都成立。
pub fn prefers_tag_format(model: &str) -> bool {
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
