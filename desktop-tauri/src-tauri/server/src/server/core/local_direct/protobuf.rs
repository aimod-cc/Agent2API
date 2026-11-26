//! 零依赖轻量 Protobuf Wire 读取器（解析 Antigravity `gen_metadata` / `steps.metadata`）。

#[derive(Clone, Debug, Default)]
pub struct ParsedGenMetadata {
    pub model: Option<String>,
    pub step_indices: Vec<(i64, i64)>,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedStepMetadata {
    pub began_ms: Option<i64>,
    pub first_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    pub uncached_input: i64,
    pub output: i64,
    pub cache_write: i64,
    pub cache_read: i64,
    pub response_id: String,
    pub gen_idx: Option<i64>,
}

#[derive(Clone, Copy)]
enum WireValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Other,
}

fn decode_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut val = 0u64;
    let mut shift = 0u32;
    for (idx, &byte) in buf.iter().take(10).enumerate() {
        val |= u64::from(byte & 0x7f) << shift;
        if (byte & 0x80) == 0 {
            return Some((val, idx + 1));
        }
        shift += 7;
    }
    None
}

fn iter_fields(mut buf: &[u8]) -> Vec<(u32, WireValue<'_>)> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let Some((tag, consumed)) = decode_varint(buf) else {
            break;
        };
        buf = &buf[consumed..];
        let wire_type = (tag & 0x07) as u8;
        let field_num = (tag >> 3) as u32;
        if field_num == 0 {
            break;
        }
        match wire_type {
            0 => {
                let Some((val, n)) = decode_varint(buf) else {
                    break;
                };
                buf = &buf[n..];
                out.push((field_num, WireValue::Varint(val)));
            }
            1 => {
                if buf.len() < 8 {
                    break;
                }
                buf = &buf[8..];
                out.push((field_num, WireValue::Other));
            }
            2 => {
                let Some((len_u64, n)) = decode_varint(buf) else {
                    break;
                };
                buf = &buf[n..];
                let Ok(len) = usize::try_from(len_u64) else {
                    break;
                };
                if buf.len() < len {
                    break;
                }
                let slice = &buf[..len];
                buf = &buf[len..];
                out.push((field_num, WireValue::Bytes(slice)));
            }
            5 => {
                if buf.len() < 4 {
                    break;
                }
                buf = &buf[4..];
                out.push((field_num, WireValue::Other));
            }
            _ => break,
        }
    }
    out
}

use base64::Engine;

/// 解析 `google.protobuf.Timestamp` 为毫秒时间戳。
pub fn parse_timestamp_ms(data: &[u8]) -> Option<i64> {
    let mut secs = 0i64;
    let mut nanos = 0i64;
    for (field, val) in iter_fields(data) {
        if let WireValue::Varint(raw) = val {
            match field {
                1 => secs = raw as i64,
                2 => nanos = raw as i64,
                _ => {}
            }
        }
    }
    if secs <= 0 {
        return None;
    }
    Some(secs.saturating_mul(1000).saturating_add(nanos / 1_000_000))
}

/// 剥离 Gemini 部署单字母后缀（如 `gemini-3.8-flash-n` -> `gemini-3.8-flash`）。
pub fn strip_gemini_deployment_suffix(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    if lower.starts_with("gemini-")
        && bytes.len() > 9
        && bytes[bytes.len() - 2] == b'-'
        && bytes[bytes.len() - 1].is_ascii_lowercase()
    {
        return trimmed[..trimmed.len() - 2].to_string();
    }
    trimmed.to_string()
}

fn parse_kv_entry(data: &[u8]) -> Option<(String, String)> {
    let mut key = None;
    let mut val = None;
    for (field, value) in iter_fields(data) {
        if let WireValue::Bytes(bytes) = value {
            let text = std::str::from_utf8(bytes).ok()?.to_string();
            match field {
                1 => key = Some(text),
                2 => val = Some(text),
                _ => {}
            }
        }
    }
    Some((key?, val.unwrap_or_default()))
}

/// 解析 `gen_metadata.data` 提取模型名称及 `(step_idx, gen_idx)` 映射。
pub fn parse_gen_metadata(gen_idx: i64, data: &[u8]) -> ParsedGenMetadata {
    let mut result = ParsedGenMetadata::default();
    let outer = iter_fields(data);
    let mut inner_slices = Vec::new();
    for (field, value) in &outer {
        if *field == 1 {
            if let WireValue::Bytes(bytes) = value {
                inner_slices.push(*bytes);
            }
        }
    }
    let target_fields = if !inner_slices.is_empty() {
        inner_slices.into_iter().flat_map(iter_fields).collect::<Vec<_>>()
    } else {
        outer
    };

    let mut f19: Option<String> = None;
    let mut f21: Option<String> = None;
    let mut f22: Option<String> = None;

    for (field, value) in target_fields {
        if let WireValue::Bytes(bytes) = value {
            match field {
                19 => {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        let s = text.trim();
                        if !s.is_empty() {
                            f19 = Some(s.to_string());
                        }
                    }
                }
                21 => {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        let s = text.trim();
                        if !s.is_empty() {
                            f21 = Some(s.to_string());
                        }
                    }
                }
                22 => {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        let s = text.trim();
                        if !s.is_empty() {
                            f22 = Some(s.to_string());
                        }
                    }
                }
                20 => {
                    if let Some((key, val)) = parse_kv_entry(bytes) {
                        if key == "last_step_index" {
                            if let Ok(idx) = val.trim().parse::<i64>() {
                                result.step_indices.push((idx + 1, gen_idx));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    result.model = f19.or(f21).or(f22);
    result
}

/// 解析 `steps.metadata`（`CortexStepMetadata`）。
pub fn parse_step_metadata(data: &[u8]) -> ParsedStepMetadata {
    let mut step = ParsedStepMetadata::default();
    for (field, value) in iter_fields(data) {
        if let WireValue::Bytes(bytes) = value {
            match field {
                1 => step.began_ms = parse_timestamp_ms(bytes),
                6 => step.first_ms = parse_timestamp_ms(bytes),
                7 => step.ended_ms = parse_timestamp_ms(bytes),
                9 => {
                    for (ufield, uval) in iter_fields(bytes) {
                        match (ufield, uval) {
                            (2, WireValue::Varint(v)) => step.uncached_input = (v as i64).max(0),
                            (3, WireValue::Varint(v)) => step.output = (v as i64).max(0),
                            (4, WireValue::Varint(v)) => step.cache_write = (v as i64).max(0),
                            (5, WireValue::Varint(v)) => step.cache_read = (v as i64).max(0),
                            (11, WireValue::Bytes(s)) => {
                                if let Ok(text) = std::str::from_utf8(s) {
                                    step.response_id = text.trim().to_string();
                                }
                            }
                            _ => {}
                        }
                    }
                }
                20 => {
                    for (gfield, gval) in iter_fields(bytes) {
                        if gfield == 3 {
                            if let WireValue::Varint(v) = gval {
                                step.gen_idx = Some(v as i64);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    step
}

/// 从 `conversation_summaries.workspace_uris` 解析工作区路径或 URI。
pub fn decode_workspace_uri(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(trimmed) {
            if let Some(first) = arr.into_iter().find(|s| !s.trim().is_empty()) {
                return first.trim().to_string();
            }
        }
    }
    if trimmed.starts_with("file://") || trimmed.starts_with('/') || trimmed.contains(":\\") {
        return trimmed.to_string();
    }
    if let Some(pos) = trimmed.find("file://") {
        let tail = &trimmed[pos..];
        let end = tail
            .find(|c: char| c.is_control() || c == '"')
            .unwrap_or(tail.len());
        return tail[..end].trim().to_string();
    }
    trimmed.to_string()
}

fn decode_b64_flexible(raw: &str) -> Option<Vec<u8>> {
    let cleaned: String = raw.chars().filter(|ch| !ch.is_ascii_whitespace()).collect();
    if cleaned.is_empty() {
        return None;
    }
    base64::engine::general_purpose::STANDARD
        .decode(&cleaned)
        .or_else(|_| {
            let unpadded = cleaned.trim_end_matches('=');
            base64::engine::general_purpose::STANDARD_NO_PAD.decode(unpadded)
        })
        .or_else(|_| {
            let unpadded = cleaned.trim_end_matches('=');
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(unpadded)
        })
        .ok()
}

pub(crate) fn is_valid_email(raw: &str) -> bool {
    let trimmed = raw.trim();
    if !(3..=254).contains(&trimmed.len()) {
        return false;
    }
    if trimmed.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return false;
    }
    let Some((local, domain)) = trimmed.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

fn email_from_inner_user_pb(bytes: &[u8]) -> Option<String> {
    let fields = iter_fields(bytes);
    for preferred_field in [3u32, 7u32] {
        for (field, value) in &fields {
            if *field == preferred_field {
                if let WireValue::Bytes(raw) = value {
                    if let Ok(text) = std::str::from_utf8(raw) {
                        let candidate = text.trim();
                        if is_valid_email(candidate) {
                            return Some(candidate.to_string());
                        }
                    }
                }
            }
        }
    }
    for (_, value) in fields {
        if let WireValue::Bytes(raw) = value {
            if let Ok(text) = std::str::from_utf8(raw) {
                let candidate = text.trim();
                if is_valid_email(candidate) {
                    return Some(candidate.to_string());
                }
            }
        }
    }
    None
}

fn try_extract_nested_b64_email(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if is_valid_email(text) {
        return Some(text.to_string());
    }
    let decoded = decode_b64_flexible(text)?;
    email_from_inner_user_pb(&decoded)
}

/// 从 Antigravity 桌面版 `state.vscdb` 的 `antigravityUnifiedStateSync.userStatus`
/// 值中提取当前登录邮箱（外层为 Base64 Protobuf，内层携带另一段 Base64 编码的用户信息）。
pub fn extract_user_status_email(raw_b64: &str) -> Option<String> {
    let outer_bytes = decode_b64_flexible(raw_b64)?;
    for (field, value) in iter_fields(&outer_bytes) {
        let WireValue::Bytes(f1_bytes) = value else {
            continue;
        };
        if field == 1 {
            for (sub_field, sub_val) in iter_fields(f1_bytes) {
                let WireValue::Bytes(sub_bytes) = sub_val else {
                    continue;
                };
                if sub_field == 2 {
                    for (_, nested_val) in iter_fields(sub_bytes) {
                        if let WireValue::Bytes(nested_bytes) = nested_val {
                            if let Some(email) = try_extract_nested_b64_email(nested_bytes) {
                                return Some(email);
                            }
                        }
                    }
                    if let Some(email) = try_extract_nested_b64_email(sub_bytes) {
                        return Some(email);
                    }
                } else if sub_field == 1 {
                    if let Some(email) = try_extract_nested_b64_email(sub_bytes) {
                        return Some(email);
                    }
                }
            }
        }
        if let Some(email) = try_extract_nested_b64_email(f1_bytes) {
            return Some(email);
        }
    }
    None
}

/// 为单元测试构造极简 Protobuf 测试载荷的辅助函数。
#[cfg(test)]
pub fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        } else {
            out.push(byte | 0x80);
        }
    }
}

#[cfg(test)]
pub fn encode_field_varint(field: u32, v: u64, out: &mut Vec<u8>) {
    encode_varint((u64::from(field) << 3) | 0, out);
    encode_varint(v, out);
}

#[cfg(test)]
pub fn encode_field_bytes(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    encode_varint((u64::from(field) << 3) | 2, out);
    encode_varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}
