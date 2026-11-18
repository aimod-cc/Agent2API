//! SSE行按UTF-8/BOM与CRLF/CR/LF增量处理；本合同比浏览器更严格拒绝未知字段。
//! 总流64MiB、单帧1MiB、16384帧，包含注释/元数据，不积攒事件队列。
use agent2api_provider_protocol as proto;
use super::super::error;

pub(super) struct Event { pub(super) name: Option<String>, pub(super) data: String }

pub(super) struct Sse {
    line: Vec<u8>,
    data: String,
    name: Option<String>,
    has_data: bool,
    first_line: bool,
    skip_lf: bool,
    event_bytes: usize,
    wire_bytes: usize,
    events: usize,
}
impl Sse {
    pub(super) fn new() -> Self {
        Self { line: Vec::new(), data: String::new(), name: None, has_data: false, first_line: true, skip_lf: false, event_bytes: 0, wire_bytes: 0, events: 0 }
    }
    pub(super) fn wire_bytes(&self) -> usize { self.wire_bytes }
    pub(super) fn events(&self) -> usize { self.events }
    pub(super) fn push(&mut self, bytes: &[u8], mut consume: impl FnMut(Event) -> Result<(), proto::WireError>) -> Result<(), proto::WireError> {
        self.wire_bytes = self.wire_bytes.checked_add(bytes.len()).filter(|n| *n <= 64 * 1024 * 1024).ok_or_else(error::protocol)?;
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    if self.event_bytes != 0 {
                        self.event_bytes = self.event_bytes.checked_add(1).filter(|n| *n <= 1024 * 1024).ok_or_else(error::protocol)?;
                    }
                    continue;
                }
            }
            self.event_bytes = self.event_bytes.checked_add(1).filter(|n| *n <= 1024 * 1024).ok_or_else(error::protocol)?;
            if matches!(byte, b'\r' | b'\n') {
                self.consume_line(&mut consume)?;
                self.skip_lf = byte == b'\r';
            } else { self.line.push(byte); }
        }
        Ok(())
    }
    fn consume_line(&mut self, consume: &mut impl FnMut(Event) -> Result<(), proto::WireError>) -> Result<(), proto::WireError> {
        let raw = std::mem::take(&mut self.line);
        let bytes = if self.first_line { raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw) } else { &raw };
        self.first_line = false;
        let line = std::str::from_utf8(bytes).map_err(|_| error::protocol())?;
        if line.is_empty() {
            self.events = self.events.checked_add(1).filter(|n| *n <= 16_384).ok_or_else(error::protocol)?;
            if self.has_data || self.name.is_some() {
                let event = Event { name: self.name.take(), data: std::mem::take(&mut self.data) };
                self.has_data = false;
                consume(event)?;
            }
            self.event_bytes = 0;
            return Ok(());
        }
        if line.starts_with(':') { return Ok(()); }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => { if self.has_data { self.data.push('\n'); } self.data.push_str(value); self.has_data = true; }
            "event" => {
                if value.len() > 128 || value.chars().any(char::is_control) || self.name.as_deref().is_some_and(|old| old != value) { return Err(error::protocol()); }
                self.name = Some(value.to_string());
            }
            "id" | "retry" => {}
            _ => return Err(error::protocol()),
        }
        Ok(())
    }
    pub(super) fn finish(&self) -> Result<(), proto::WireError> {
        // EOF不得将残行/残data/注释/元数据派发或补空行。
        if !self.line.is_empty() || self.has_data || self.name.is_some() || self.event_bytes != 0 { Err(error::protocol()) } else { Ok(()) }
    }
}
