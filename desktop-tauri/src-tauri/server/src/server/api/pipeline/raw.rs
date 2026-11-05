//! 调试模式下的下游正文采集，单条大小上限与上游调试报文保持一致。

const MAX_RAW_BYTES: usize = crate::server::core::debug_traffic::MAX_BODY_BYTES;

pub fn raw_body_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.len() > MAX_RAW_BYTES {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

pub fn capture_raw_body(bytes: &[u8]) -> Option<String> {
    if !crate::server::core::debug_traffic::enabled() {
        return None;
    }
    raw_body_text(bytes)
}

pub fn capture_raw_json<T: serde::Serialize>(value: &T) -> Option<String> {
    if !crate::server::core::debug_traffic::enabled() {
        return None;
    }
    let text = serde_json::to_string(value).ok()?;
    (text.len() <= MAX_RAW_BYTES).then_some(text)
}

pub(super) struct RawCapture {
    enabled: bool,
    buf: Vec<u8>,
}

impl RawCapture {
    pub(super) fn new(enabled: bool) -> Self {
        Self { enabled, buf: Vec::new() }
    }

    pub(super) fn push(&mut self, chunk: &[u8]) {
        if self.enabled && self.buf.len().saturating_add(chunk.len()) <= MAX_RAW_BYTES {
            self.buf.extend_from_slice(chunk);
        } else if self.enabled {
            // 超过上限后停止累积，避免超大响应流把整份正文复制进内存
            self.enabled = false;
            self.buf.clear();
        }
    }

    pub(super) fn into_text(&mut self) -> Option<String> {
        if !self.enabled || self.buf.is_empty() {
            return None;
        }
        // UTF-8 字符可能跨分片，收尾后才解码。
        let bytes = std::mem::take(&mut self.buf);
        Some(match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RawCapture, MAX_RAW_BYTES};

    #[test]
    fn oversized_stream_stops_accumulating() {
        let mut capture = RawCapture::new(true);
        let chunk = vec![b'x'; MAX_RAW_BYTES + 1];
        capture.push(&chunk);
        assert!(capture.into_text().is_none());
    }
}
