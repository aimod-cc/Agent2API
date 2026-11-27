//! 调试模式下的下游正文采集，不对日志副本设置单条大小上限。

pub fn raw_body_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

pub fn capture_raw_body(bytes: &[u8]) -> Option<String> {
    if !crate::server::core::debug_traffic::enabled() {
        return None;
    }
    let privacy = crate::server::config::current().privacy_settings();
    if privacy.settings.enabled {
        let safe = crate::server::core::privacy::redact_debug_body(bytes, "", &privacy);
        return raw_body_text(&safe);
    }
    raw_body_text(bytes)
}

pub fn capture_raw_json<T: serde::Serialize>(value: &T) -> Option<String> {
    if !crate::server::core::debug_traffic::enabled() {
        return None;
    }
    serde_json::to_string(value).ok()
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
        if self.enabled {
            self.buf.extend_from_slice(chunk);
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
