use serde_json::Value;

use crate::server::config::ImageCompressSettings;
use crate::server::core::protocol::native::WireProtocol;

use super::{compress_with_cache, ImageCompressStats};

pub fn compress_native_images(
    body: &mut Value,
    protocol: WireProtocol,
    settings: &ImageCompressSettings,
) -> ImageCompressStats {
    let key = match protocol {
        WireProtocol::Chat | WireProtocol::Anthropic => "messages",
        WireProtocol::Responses => "input",
    };
    let mut stats = ImageCompressStats::default();
    if let Some(messages) = body.get_mut(key).and_then(Value::as_array_mut) {
        for message in messages {
            if let Some(content) = message.get_mut("content") {
                compress_content(content, settings, &mut stats);
            }
            if let Some(output) = message.get_mut("output") {
                compress_content(output, settings, &mut stats);
            }
        }
    }
    stats
}

fn compress_content(
    content: &mut Value,
    settings: &ImageCompressSettings,
    stats: &mut ImageCompressStats,
) {
    let Some(parts) = content.as_array_mut() else {
        return;
    };
    for part in parts {
        match part.get("type").and_then(Value::as_str).unwrap_or("") {
            "image" => {
                let Some(source) = part.get_mut("source") else {
                    continue;
                };
                if source.get("type").and_then(Value::as_str) != Some("base64") {
                    continue;
                }
                let Some(media) = source.get("media_type").and_then(Value::as_str) else {
                    continue;
                };
                let Some(data) = source.get("data").and_then(Value::as_str) else {
                    continue;
                };
                let uri = format!("data:{media};base64,{data}");
                let Some(next) = compress_with_cache(&uri, settings) else {
                    continue;
                };
                let Some((prefix, data)) = next.split_once(";base64,") else {
                    continue;
                };
                let Some(media) = prefix.strip_prefix("data:") else {
                    continue;
                };
                if let Some(object) = source.as_object_mut() {
                    object.insert("media_type".into(), media.into());
                    object.insert("data".into(), data.into());
                    note(stats, uri.len(), next.len());
                }
            }
            "input_image" | "image_url" => {
                let Some(field) = part.get_mut("image_url") else {
                    continue;
                };
                let value = if field.is_object() {
                    field.get_mut("url")
                } else {
                    Some(field)
                };
                let Some(Value::String(uri)) = value else {
                    continue;
                };
                if let Some(next) = compress_with_cache(uri, settings) {
                    note(stats, uri.len(), next.len());
                    *uri = next;
                }
            }
            "tool_result" => {
                if let Some(content) = part.get_mut("content") {
                    compress_content(content, settings, stats);
                }
            }
            _ => {}
        }
    }
}

fn note(stats: &mut ImageCompressStats, before: usize, after: usize) {
    stats.compressed += 1;
    stats.bytes_before = stats.bytes_before.saturating_add(before);
    stats.bytes_after = stats.bytes_after.saturating_add(after);
}
