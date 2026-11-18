//! 规范 Chat 图片纯预检与原内容块移交；不访问账号、图片 URL 或文件。

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};

use super::{format, invalid, limit, MAX_IMAGES, MAX_INLINE_IMAGES, MAX_TOTAL_BYTES};

// 与封存入口统一固定长度；预算占位与最终宿主标识完全同长。
const PLACEHOLDER_ID: &str = "blob_00000000000000000000000000000000";

#[derive(Clone, Copy)]
pub(super) struct Position {
    message: usize,
    part: usize,
}

#[derive(Clone, Copy)]
enum Source {
    DataUrl,
    Base64,
    SourceUrl,
}

struct Entry {
    position: Position,
    source: Source,
    format: format::Format,
    size: usize,
}

pub(super) struct PendingImage {
    entry: Entry,
    // 从原 params 移出，不克隆大 base64。字段私有，不能外部更换待解码内容。
    block: Value,
}

impl PendingImage {
    pub(super) fn position(&self) -> Position {
        self.entry.position
    }
    pub(super) fn format(&self) -> format::Format {
        self.entry.format
    }
    pub(super) fn size(&self) -> usize {
        self.entry.size
    }
    pub(super) fn encoded(&self) -> Result<&str, proto::WireError> {
        match self.entry.source {
            Source::DataUrl => data_url(image_url(&self.block)?)?
                .map(|(_, encoded)| encoded)
                .ok_or_else(|| invalid("DeepSeek 内联图片准备内容已改变")),
            Source::Base64 => self
                .block
                .pointer("/source/data")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("DeepSeek 内联图片准备内容已改变")),
            Source::SourceUrl => data_url(
                self.block
                    .pointer("/source/url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("DeepSeek 内联图片准备内容已改变"))?,
            )?
            .map(|(_, encoded)| encoded)
            .ok_or_else(|| invalid("DeepSeek 内联图片准备内容已改变")),
        }
    }
}

fn image_url(block: &Value) -> Result<&str, proto::WireError> {
    let source = block.get("image_url").unwrap_or(block);
    source
        .as_str()
        .or_else(|| source.get("url").and_then(Value::as_str))
        .filter(|url| !url.is_empty())
        .ok_or_else(|| invalid("DeepSeek 图片缺少有效 data URL 或公共图片 URL"))
}

fn data_url(url: &str) -> Result<Option<(&str, &str)>, proto::WireError> {
    let Some(value) = url.strip_prefix("data:") else {
        return Ok(None);
    };
    let (metadata, encoded) = value
        .split_once(',')
        .ok_or_else(|| invalid("DeepSeek 内联图片 data URL 无效"))?;
    let mut fields = metadata.split(';');
    let mime = fields.next().unwrap_or("");
    if fields.next() != Some("base64") || fields.next().is_some() {
        return Err(invalid(
            "DeepSeek 内联图片必须明确使用 data:image/格式;base64,内容",
        ));
    }
    Ok(Some((mime, encoded)))
}

fn inline(block: &Value) -> Result<Option<(Source, &str, &str)>, proto::WireError> {
    match block.get("type").and_then(Value::as_str) {
        Some("image_url" | "input_image") => Ok(
            data_url(image_url(block)?)?.map(|(mime, encoded)| (Source::DataUrl, mime, encoded))
        ),
        Some("image") => {
            let source = &block["source"];
            match source["type"].as_str() {
                Some("base64") => {
                    let mime = source["media_type"]
                        .as_str()
                        .ok_or_else(|| invalid("DeepSeek 内联图片缺少明确类型，未默认当作 PNG"))?;
                    let encoded = source["data"]
                        .as_str()
                        .ok_or_else(|| invalid("DeepSeek 内联图片缺少有效 base64 内容"))?;
                    Ok(Some((Source::Base64, mime, encoded)))
                }
                Some("url") => {
                    let url = source["url"]
                        .as_str()
                        .filter(|url| !url.is_empty())
                        .ok_or_else(|| invalid("DeepSeek 图片缺少有效 URL"))?;
                    Ok(data_url(url)?.map(|(mime, encoded)| (Source::SourceUrl, mime, encoded)))
                }
                _ => Err(invalid("DeepSeek 图片来源类型尚未接入，未降级为文本")),
            }
        }
        _ => Ok(None),
    }
}

/// 先完整计算数量/单图/总量，成功后才移出大字符串；此处没有大图分配或资源登记。
pub(super) async fn extract(
    params: &mut Value,
    enabled: bool,
) -> Result<Vec<PendingImage>, proto::WireError> {
    reject_resource_blocks(params).await?;
    let messages = params
        .pointer("/body/messages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("DeepSeek 图片入场只接受规范 Chat messages"))?;
    let mut entries = Vec::new();
    let mut total = 0usize;
    let mut count = 0usize;
    let mut visited = 0usize;
    for (message_index, message) in messages.iter().enumerate() {
        visited += 1;
        if visited % 256 == 0 {
            tokio::task::yield_now().await;
        }
        let Some(parts) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (part_index, block) in parts.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) == Some("image_resource") {
                return Err(invalid(
                    "图片资源标识只能由本次主程序生成，不接受客户端提供的 image_resource",
                ));
            }
            if matches!(
                block.get("type").and_then(Value::as_str),
                Some("image_url" | "input_image" | "image")
            ) {
                count += 1;
                if count > MAX_IMAGES {
                    return Err(limit("DeepSeek 单次最多接受 50 张图片，未继续准备"));
                }
                if enabled {
                    if message.get("role").and_then(Value::as_str) != Some("user") {
                        return Err(invalid("DeepSeek 图片只能放在 user 消息中"));
                    }
                    if let Some((source, mime, encoded)) = inline(block)? {
                        if entries.len() >= MAX_INLINE_IMAGES {
                            return Err(limit("DeepSeek 本次入场最多 15 张内联图片；原 16 个资源槽保留一个，未提高上限"));
                        }
                        let (format, size) = format::preflight(mime, encoded)?;
                        total = total
                            .checked_add(size)
                            .filter(|size| *size <= MAX_TOTAL_BYTES)
                            .ok_or_else(|| limit("DeepSeek 内联图片累计超过 64 MiB 资源上限"))?;
                        entries.push(Entry {
                            position: Position {
                                message: message_index,
                                part: part_index,
                            },
                            source,
                            format,
                            size,
                        });
                    }
                }
            }
            visited += 1;
            if visited % 256 == 0 {
                tokio::task::yield_now().await;
            }
        }
    }
    let mut pending = Vec::with_capacity(entries.len());
    for entry in entries {
        let slot = slot(params, entry.position)?;
        let placeholder = json!({ "type": "image_resource", "resource": PLACEHOLDER_ID,
            "mime": entry.format.mime(), "size": entry.size });
        pending.push(PendingImage {
            entry,
            block: std::mem::replace(slot, placeholder),
        });
    }
    Ok(pending)
}

// 只扫结构、不复制字符串。拒绝资源块也覆盖嵌套附件，避免新能力关闭时静默带入标识。
async fn reject_resource_blocks(root: &Value) -> Result<(), proto::WireError> {
    enum Cursor<'a> {
        One(&'a Value),
        Array(std::slice::Iter<'a, Value>),
        Object(serde_json::map::Values<'a>),
    }
    let mut stack = vec![Cursor::One(root)];
    let mut visited = 0usize;
    while let Some(cursor) = stack.pop() {
        let value = match cursor {
            Cursor::One(value) => value,
            Cursor::Array(mut values) => {
                let Some(value) = values.next() else {
                    continue;
                };
                stack.push(Cursor::Array(values));
                value
            }
            Cursor::Object(mut values) => {
                let Some(value) = values.next() else {
                    continue;
                };
                stack.push(Cursor::Object(values));
                value
            }
        };
        if value.get("type").and_then(Value::as_str) == Some("image_resource") {
            return Err(invalid(
                "图片资源标识只能由本次主程序生成，不接受客户端提供的 image_resource",
            ));
        }
        if stack.len() >= 128 {
            return Err(limit("DeepSeek 请求嵌套超过图片入场安全上限"));
        }
        match value {
            Value::Array(values) => stack.push(Cursor::Array(values.iter())),
            Value::Object(values) => stack.push(Cursor::Object(values.values())),
            _ => {}
        }
        visited += 1;
        if visited > 1_000_000 {
            return Err(limit("DeepSeek 请求条目超过图片入场安全上限"));
        }
        if visited % 256 == 0 {
            tokio::task::yield_now().await;
        }
    }
    Ok(())
}

fn slot(params: &mut Value, position: Position) -> Result<&mut Value, proto::WireError> {
    params
        .get_mut("body")
        .and_then(|body| body.get_mut("messages"))
        .and_then(Value::as_array_mut)
        .and_then(|messages| messages.get_mut(position.message))
        .and_then(|message| message.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|parts| parts.get_mut(position.part))
        .ok_or_else(|| invalid("DeepSeek 图片准备位置已改变"))
}

pub(super) fn replace(
    params: &mut Value,
    position: Position,
    block: Value,
) -> Result<(), proto::WireError> {
    *slot(params, position)? = block;
    Ok(())
}
