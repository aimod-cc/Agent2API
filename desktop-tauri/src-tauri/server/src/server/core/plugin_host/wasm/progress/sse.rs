const MAX_EVENT_BYTES: usize = 1024 * 1024;

pub(super) struct Payload {
    sse: bool,
    line: Vec<u8>,
    data: Vec<u8>,
    event_bytes: usize,
    skipping: bool,
    line_bytes: usize,
    heartbeat: bool,
    skip_lf: bool,
    memory: crate::server::core::runtime_capacity::MemoryBudget,
}

impl Payload {
    pub(super) fn new(sse: bool) -> Self {
        Self { sse, line: Vec::new(), data: Vec::new(), event_bytes: 0, skipping: false,
            line_bytes: 0, heartbeat: false, skip_lf: false, memory: Default::default() }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> u64 {
        if !self.sse { return bytes.len() as u64; }
        if self.memory.resize(MAX_EVENT_BYTES * 4).is_err() {
            self.skipping = true;
            self.line.clear();
            self.data.clear();
            return 0;
        }
        let mut payload = 0;
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' { continue; }
            }
            if matches!(byte, b'\r' | b'\n') {
                payload += self.line();
                self.skip_lf = byte == b'\r';
            } else {
                self.line_bytes = self.line_bytes.saturating_add(1);
                let comment = self.line.first() == Some(&b':') || self.line_bytes == 1 && byte == b':';
                if !comment {
                    self.event_bytes = self.event_bytes.saturating_add(1);
                    if self.event_bytes > MAX_EVENT_BYTES {
                        self.skipping = true;
                        self.line.clear();
                        self.data.clear();
                    }
                }
                if !self.skipping && (!comment || self.line.is_empty()) { self.line.push(byte); }
            }
        }
        self.memory.shrink(self.memory.bytes().saturating_sub(self.line.capacity().saturating_add(self.data.capacity())));
        payload
    }

    fn line(&mut self) -> u64 {
        if self.skipping {
            if self.line_bytes == 0 {
                self.skipping = false;
                self.heartbeat = false;
                self.event_bytes = 0;
            }
            self.line.clear();
            self.data.clear();
            self.line_bytes = 0;
            return 0;
        }
        let mut bytes = 0;
        if self.line_bytes == 0 {
            bytes = if self.heartbeat { self.data.clear(); 0 } else { self.data() };
            self.heartbeat = false;
            self.event_bytes = 0;
        } else if let Some(event) = self.line.strip_prefix(b"event:") {
            self.heartbeat = std::str::from_utf8(event).ok().is_some_and(|event| heartbeat(event.trim()));
        } else if let Some(data) = self.line.strip_prefix(b"data:") {
            self.event_bytes = self.event_bytes.saturating_add(1);
            if self.event_bytes > MAX_EVENT_BYTES {
                self.skipping = true;
                self.data.clear();
            } else {
                if !self.data.is_empty() { self.data.push(b'\n'); }
                self.data.extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
            }
        }
        self.line.clear();
        self.line_bytes = 0;
        bytes
    }

    fn data(&mut self) -> u64 {
        let bytes = if meaningful(&self.data) { self.data.len() as u64 } else { 0 };
        self.data.clear();
        bytes
    }

    pub(super) fn finish(&mut self) -> u64 {
        if !self.sse || self.skipping { return 0; }
        let bytes = self.line();
        bytes + if self.skipping || self.heartbeat { 0 } else { self.data() }
    }
}

fn heartbeat(text: &str) -> bool {
    text.len() <= 16 && matches!(text.to_ascii_lowercase().as_str(), "ping" | "pong" | "heartbeat" | "keepalive" | "keep-alive")
}

pub(super) fn meaningful(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else { return false; };
    let text = text.trim();
    if text.is_empty() || matches!(text, "[DONE]" | "[KEEPALIVE]" | "null") { return false; }
    if heartbeat(text) { return false; }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if value.is_null() || value.as_object().is_some_and(|fields| fields.is_empty())
            || value.as_array().is_some_and(|items| items.is_empty()) { return false; }
        let kind = value["type"].as_str().or_else(|| value["event"].as_str()).unwrap_or("");
        if heartbeat(kind) { return false; }
        if let Some(choices) = value["choices"].as_array() {
            return value["usage"].as_object().is_some_and(|usage| !usage.is_empty()) || choices.iter().any(|choice| {
                choice["finish_reason"].as_str().is_some_and(|reason| !reason.is_empty())
                    || ["delta", "message"].iter().any(|field| choice[*field].as_object().is_some_and(|fields|
                        fields.iter().any(|(name, value)| name != "role" && !value.is_null()
                            && !value.as_str().is_some_and(|text| text.is_empty())
                            && !value.as_array().is_some_and(|items| items.is_empty())
                            && !value.as_object().is_some_and(|fields| fields.is_empty()))))
            });
        }
    }
    true
}
